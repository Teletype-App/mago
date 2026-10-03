use crate::plugin::{Plugin, PluginMeta, PluginRegistry};

pub struct Yii2Plugin;

impl Plugin for Yii2Plugin {
    fn meta(&self) -> &'static PluginMeta {
        static META: PluginMeta = PluginMeta::new(
            "yii2",
            "Yii 2",
            "Native throws inference for Yii lifecycle and implicit calls",
            &["yii"],
            false,
        );
        &META
    }

    fn register(&self, registry: &mut PluginRegistry) {
        registry.yii2_throws = true;
    }
}
