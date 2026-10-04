use std::collections::BTreeMap;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use foldhash::{HashMap, HashSet};
use mago_analyzer::throws::{FunctionThrowsSummary, ThrowsContext, ThrowsSummaries};
use mago_codex::identifier::function_like::FunctionLikeIdentifier;
use mago_codex::ttype::union::TUnion;
use mago_database::file::{FileId, FileType};
use mago_database::{DatabaseReader, ReadDatabase};
use serde::{Deserialize, Serialize};

const SCHEMA: u32 = 9;
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Input {
    digest: String,
    kind: FileType,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    yii2: mago_analyzer::throws::yii2::Facts,
    schema: u32,
    environment: String,
    inputs: Vec<(FileId, Input)>,
    functions: Vec<(FunctionLikeIdentifier, FunctionThrowsSummary)>,
    contexts: Vec<(StoredContext, FunctionThrowsSummary)>,
    dependencies: Vec<(FileId, Vec<FileId>)>,
}

// TUnion contains maps with enum and tuple keys, including PHP array shapes.
// Preserve its complete type information with the same codec used by the prelude.
#[derive(Serialize, Deserialize)]
struct StoredContext {
    function: FunctionLikeIdentifier,
    arguments: Vec<(usize, Vec<u8>)>,
}

impl StoredContext {
    fn encode(context: &ThrowsContext) -> Result<Self, bincode::error::EncodeError> {
        let arguments = context
            .arguments
            .iter()
            .map(|(index, value)| {
                bincode::serde::encode_to_vec(value, bincode::config::standard()).map(|bytes| (*index, bytes))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { function: context.function, arguments })
    }

    fn decode(self) -> Option<ThrowsContext> {
        let arguments = self
            .arguments
            .into_iter()
            .map(|(index, bytes)| {
                let (value, consumed) = bincode::serde::decode_from_slice::<TUnion, _>(
                    &bytes,
                    bincode::config::standard().with_limit::<{ MAX_CACHE_BYTES as usize }>(),
                )
                .ok()?;
                (consumed == bytes.len()).then_some((index, value))
            })
            .collect::<Option<_>>()?;
        Some(ThrowsContext { function: self.function, arguments })
    }
}

pub(super) struct ThrowsCache {
    path: PathBuf,
    environment: String,
    inputs: BTreeMap<FileId, Input>,
}

impl ThrowsCache {
    pub fn new(path: PathBuf, environment: String, database: &ReadDatabase) -> Self {
        let inputs = database
            .files()
            .map(|file| {
                (
                    file.id,
                    Input { digest: blake3::hash(file.contents.as_ref()).to_hex().to_string(), kind: file.file_type },
                )
            })
            .collect();
        Self { path, environment, inputs }
    }

    pub fn seed(&self) -> Option<(ThrowsSummaries, HashSet<FileId>)> {
        let mut file = std::fs::File::open(&self.path).ok()?;
        if file.metadata().ok()?.len() > MAX_CACHE_BYTES {
            return None;
        }
        let mut content = Vec::new();
        Read::by_ref(&mut file).take(MAX_CACHE_BYTES + 1).read_to_end(&mut content).ok()?;
        if content.len() as u64 > MAX_CACHE_BYTES {
            return None;
        }
        let stored: Stored = serde_json::from_slice(&content).ok()?;
        let stored_inputs = stored.inputs.into_iter().collect::<BTreeMap<_, _>>();
        if stored.schema != SCHEMA
            || stored.environment != self.environment
            || !stored_inputs.keys().eq(self.inputs.keys())
        {
            return None;
        }
        let mut affected = HashSet::default();
        for (id, input) in &self.inputs {
            if stored_inputs.get(id) == Some(input) {
                continue;
            }
            if input.kind != FileType::Host {
                return None;
            }
            affected.insert(*id);
        }
        let dependencies = stored
            .dependencies
            .into_iter()
            .map(|(id, dependencies)| (id, dependencies.into_iter().collect::<HashSet<_>>()))
            .collect::<HashMap<_, _>>();
        loop {
            let callers = dependencies
                .iter()
                .filter(|(file, callees)| !affected.contains(file) && !callees.is_disjoint(&affected))
                .map(|(file, _)| *file)
                .collect::<Vec<_>>();
            if callers.is_empty() {
                break;
            }
            affected.extend(callers);
        }
        tracing::debug!(affected_files = affected.len(), "Reusing throws summaries after dependency invalidation");
        Some((
            ThrowsSummaries {
                yii2: stored.yii2,
                source_files: self
                    .inputs
                    .iter()
                    .filter(|(_, input)| input.kind == FileType::Host)
                    .map(|(id, _)| *id)
                    .collect(),
                functions: stored.functions.into_iter().collect(),
                contexts: stored
                    .contexts
                    .into_iter()
                    .map(|(context, summary)| context.decode().map(|context| (context, summary)))
                    .collect::<Option<_>>()?,
                dependencies,
            },
            affected,
        ))
    }

    pub fn save(&self, summaries: &ThrowsSummaries) {
        let started = tracing::enabled!(tracing::Level::TRACE).then(std::time::Instant::now);
        if let Err(error) = self.write(summaries) {
            tracing::warn!("Cannot write throws cache: {error}");
        }
        if let Some(started) = started {
            tracing::trace!(elapsed = ?started.elapsed(), "Throws cache write completed");
        }
    }

    fn write(&self, summaries: &ThrowsSummaries) -> Result<(), Box<dyn std::error::Error>> {
        let parent =
            self.path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let stored = Stored {
            yii2: summaries.yii2.clone(),
            schema: SCHEMA,
            environment: self.environment.clone(),
            inputs: self.inputs.iter().map(|(id, input)| (*id, input.clone())).collect(),
            functions: summaries.functions.iter().map(|(id, summary)| (*id, summary.clone())).collect(),
            contexts: summaries
                .contexts
                .iter()
                .map(|(context, summary)| StoredContext::encode(context).map(|context| (context, summary.clone())))
                .collect::<Result<_, _>>()?,
            dependencies: summaries
                .dependencies
                .iter()
                .map(|(id, dependencies)| (*id, dependencies.iter().copied().collect()))
                .collect(),
        };
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut writer = BufWriter::new(temporary.as_file_mut());
            serde_json::to_writer(&mut writer, &stored)?;
            writer.flush()?;
        }
        temporary.persist(&self.path)?;
        Ok(())
    }
}
