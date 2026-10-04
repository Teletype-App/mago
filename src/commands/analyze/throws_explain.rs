use std::io::{BufWriter, Write};
use std::path::Path;

use mago_analyzer::throws::{ConditionValue, FunctionThrowsSummary, ThrowsSummaries};
use mago_codex::ttype::TType;
use mago_database::{DatabaseReader, ReadDatabase};
use mago_span::Span;
use serde_json::{Value, json};

use crate::error::Error;

fn location(span: Span, database: &ReadDatabase) -> Value {
    let file = database.get(&span.file_id).ok();
    json!({"file": file.as_ref().map(|file| String::from_utf8_lossy(&file.name).into_owned()), "line": file.as_ref().map(|file| file.line_number(span.start.offset) + 1), "offset": span.start.offset})
}

fn summary(throws: &FunctionThrowsSummary, database: &ReadDatabase) -> Value {
    let mut exceptions = throws.exceptions.iter().collect::<Vec<_>>();
    exceptions.sort_by_key(|(name, _)| **name);
    let exceptions = exceptions.into_iter().map(|(name, alternatives)| {
        let conditions = alternatives.iter().map(|conjunction| conjunction.iter().map(|(index, value)| {
            let value = match value { ConditionValue::Bool(value) => json!(value), ConditionValue::Int(value) => json!(value), ConditionValue::String(value) => json!(value.to_string()), ConditionValue::Null => Value::Null };
            json!({"parameter_index": index, "equals": value})
        }).collect::<Vec<_>>()).collect::<Vec<_>>();
        let sites = throws.provenance.get(name).into_iter().flatten().map(|site| json!({"location": location(site.span, database), "callee": site.target.map(|target| target.as_string())})).collect::<Vec<_>>();
        json!({"exception": name.to_string(), "conditions": conditions, "origins": sites})
    }).collect::<Vec<_>>();
    let mut unresolved = throws.unresolved_calls.iter().copied().collect::<Vec<_>>();
    unresolved.sort();
    let mut result = json!({"exceptions": exceptions, "unresolved_calls": unresolved.into_iter().map(|span| location(span, database)).collect::<Vec<_>>()});
    if let Some(returned) = &throws.returned_generator {
        result["returned_generator"] = summary(returned, database);
    }
    result
}

pub(super) fn write(path: &Path, summaries: &ThrowsSummaries, database: &ReadDatabase) -> Result<(), Error> {
    let mut functions = summaries.functions.iter().collect::<Vec<_>>();
    functions.sort_by_key(|(id, _)| **id);
    let mut contexts = summaries.contexts.iter().collect::<Vec<_>>();
    contexts.sort_by_key(|(key, _)| (*key).clone());
    let document = json!({
        "schema": 1,
        "functions": functions.into_iter().map(|(id, throws)| json!({"function": id.as_string(), "summary": summary(throws, database)})).collect::<Vec<_>>(),
        "contexts": contexts.into_iter().map(|(key, throws)| json!({"function": key.function.as_string(), "arguments": key.arguments.iter().map(|(index, value)| json!({"parameter_index": index, "type": value.get_id()})).collect::<Vec<_>>(), "summary": summary(throws, database)})).collect::<Vec<_>>(),
    });
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::InvalidArgument(format!("Cannot create throws explanation directory: {error}")))?;
    let temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| Error::InvalidArgument(format!("Cannot create throws explanation: {error}")))?;
    {
        let mut writer = BufWriter::new(temporary.as_file());
        serde_json::to_writer_pretty(&mut writer, &document)?;
        writer.flush().map_err(|error| Error::InvalidArgument(format!("Cannot write throws explanation: {error}")))?;
    }
    temporary
        .persist(path)
        .map_err(|error| Error::InvalidArgument(format!("Cannot save throws explanation: {error}")))?;
    Ok(())
}
