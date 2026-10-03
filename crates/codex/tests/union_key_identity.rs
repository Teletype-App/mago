use std::borrow::Cow;
use std::collections::HashMap;

use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::get_list;
use mago_codex::ttype::union::TUnion;

fn union(types: Vec<TAtomic>) -> TUnion {
    TUnion::new(Cow::Owned(types))
}

#[test]
fn equal_union_types_retrieve_the_same_cached_value() {
    for (stored, requested) in [
        (union(vec![TAtomic::Null, TAtomic::Void]), union(vec![TAtomic::Void, TAtomic::Null])),
        (
            union(vec![TAtomic::Null, TAtomic::Null, TAtomic::Void]),
            union(vec![TAtomic::Null, TAtomic::Void, TAtomic::Void]),
        ),
        (get_list(union(vec![TAtomic::Null, TAtomic::Void])), get_list(union(vec![TAtomic::Void, TAtomic::Null]))),
    ] {
        assert_eq!(stored, requested);
        let cache = HashMap::from([(stored, "computed summary")]);
        assert_eq!(cache.get(&requested), Some(&"computed summary"));
    }
}
