//! Plugin translations with Project Fluent: a plugin folder may hold `locales/<lang>.ftl`;
//! the manifest's strings are the source text and the fallback for a missing file or message
//! (docs/superpowers/specs/2026-11-09-extensions-stats-design.md §3).

use std::sync::{Arc, OnceLock};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::FluentResource;
use unic_langid::LanguageIdentifier;

use crate::i18n::{lang, Lang};
use crate::plugin::{Action, Kind, Plugin, Setting};

type Bundle = FluentBundle<FluentResource>;

/// A plugin's bundles, one per app language, each read on first use. Clones share them.
#[derive(Clone, Default)]
pub struct Locales(Arc<[OnceLock<Option<Bundle>>; 2]>);

impl std::fmt::Debug for Locales {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Locales")
    }
}

impl Plugin {
    /// `locales/<code>.ftl` as a bundle; `None` without the file. Parse errors are logged and
    /// the messages Fluent could read are kept.
    fn bundle(&self, lang: Lang) -> Option<&Bundle> {
        self.locales.0[lang as usize]
            .get_or_init(|| {
                let path = self.user_dir().join("locales").join(format!("{}.ftl", lang.code()));
                let source = std::fs::read_to_string(&path).ok()?;
                let resource = FluentResource::try_new(source).unwrap_or_else(|(partial, errors)| {
                    eprintln!("plugin {}: {}: {errors:?}", self.id, path.display());
                    partial
                });
                let locale: LanguageIdentifier = lang.code().parse().unwrap_or_default();
                let mut bundle = Bundle::new_concurrent(vec![locale]);
                // No Unicode isolation marks around placeables: the fonts would draw them.
                bundle.set_use_isolating(false);
                if let Err(errors) = bundle.add_resource(resource) {
                    eprintln!("plugin {}: {}: {errors:?}", self.id, path.display());
                }
                Some(bundle)
            })
            .as_ref()
    }

    /// Message `id` in `lang`, if the plugin translates it.
    fn message(&self, lang: Lang, id: &str) -> Option<String> {
        let bundle = self.bundle(lang)?;
        let pattern = bundle.get_message(id)?.value()?;
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, None, &mut errors).into_owned();
        if !errors.is_empty() {
            eprintln!("plugin {}: {id}: {errors:?}", self.id);
        }
        Some(text)
    }

    fn text_in(&self, lang: Lang, id: &str, fallback: &str) -> String {
        self.message(lang, id).unwrap_or_else(|| fallback.to_string())
    }

    /// Message `id` in the app's language, else `fallback` (the manifest's string).
    pub fn text(&self, id: &str, fallback: &str) -> String {
        self.text_in(lang(), id, fallback)
    }

    pub fn display_name(&self) -> String {
        self.text("display-name", self.name())
    }

    /// The description, or why the manifest can't be used.
    pub fn description(&self) -> String {
        match &self.manifest {
            Ok(m) => self.text("description", &m.description),
            Err(e) => e.clone(),
        }
    }

    pub fn action_label(&self, action: &Action) -> String {
        self.text(&format!("action-{}", action.key()), &action.menu_label)
    }

    pub fn setting_label(&self, setting: &Setting) -> String {
        self.text(&format!("setting-{}", setting.id), &setting.label)
    }

    /// What a list setting shows when empty, if the plugin says.
    pub fn setting_empty(&self, setting: &Setting) -> Option<String> {
        let Kind::List { empty_label, .. } = &setting.kind else { return None };
        self.message(lang(), &format!("setting-{}-empty", setting.id)).or_else(|| empty_label.clone())
    }

    /// A choice option's shown text; the stored value stays `value`.
    pub fn option_label(&self, setting: &Setting, value: &str) -> String {
        self.text(&format!("setting-{}-option-{value}", setting.id), value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{discover, Dirs};
    use std::path::PathBuf;

    const MANIFEST: &str = r#"
display_name = "Echo"
description = "Says things"
plugin_version = "1.0.0"
format_version = 1

[status]
executable = "status"

[[action]]
menu_label = "Save diagnostics"
executable = "bin/diagnose"

[[action]]
id = "hello"
menu_label = "Say hello"
executable = "say"
"#;

    /// A plugin in a scratch folder with the given `locales/` files.
    fn plugin(name: &str, locales: &[(&str, &str)]) -> (PathBuf, Plugin) {
        let root = std::env::temp_dir().join(format!("zarathustra-plugin-i18n-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("installed/echo");
        std::fs::create_dir_all(dir.join("locales")).expect("dirs");
        std::fs::write(dir.join("plugin.toml"), MANIFEST).expect("manifest");
        std::fs::write(dir.join("status"), "#!/bin/sh\necho \"Language=$ZARATHUSTRA_LANGUAGE\"\n").expect("status");
        for (code, ftl) in locales {
            std::fs::write(dir.join(format!("locales/{code}.ftl")), ftl).expect("ftl");
        }
        let dirs = Dirs {
            installed: root.join("installed"),
            runtime: Some(root.join("runtime")),
            data: root.join("data"),
            database: root.join("zarathustra.db"),
        };
        let p = discover(&dirs).into_iter().next().expect("plugin");
        (root, p)
    }

    #[test]
    fn uses_the_language_file() {
        let (root, p) = plugin("lang", &[("it", "action-hello = Saluta\ndisplay-name = Eco\n")]);
        let hello = &p.manifest.as_ref().expect("manifest").action[1];
        assert_eq!(p.text_in(Lang::It, "action-hello", &hello.menu_label), "Saluta");
        assert_eq!(p.text_in(Lang::It, "display-name", p.name()), "Eco");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn falls_back_to_manifest() {
        let (root, p) = plugin("fallback", &[("it", "display-name = Eco\n")]);
        assert_eq!(p.text_in(Lang::It, "description", "Says things"), "Says things", "missing id");
        assert_eq!(p.text_in(Lang::En, "display-name", "Echo"), "Echo", "no en.ftl");
        let (root2, bare) = plugin("bare", &[]);
        assert_eq!(bare.text_in(Lang::It, "display-name", "Echo"), "Echo", "no file");
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(root2).ok();
    }

    #[test]
    fn broken_ftl_falls_back() {
        let (root, p) = plugin("broken", &[("it", "this is not fluent {\n= nor this\ndescription = Dice cose\n")]);
        assert_eq!(p.text_in(Lang::It, "description", "Says things"), "Dice cose");
        assert_eq!(p.text_in(Lang::It, "this", "fallback"), "fallback");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn action_key_defaults_to_executable() {
        let m: crate::plugin::Manifest = toml::from_str(MANIFEST).expect("parses");
        assert_eq!(m.action[0].key(), "diagnose");
        assert_eq!(m.action[1].key(), "hello");
    }

    #[test]
    fn language_env_set() {
        let (root, p) = plugin("env", &[]);
        p.prepare().expect("prepare");
        let want = lang().code();
        assert_eq!(p.status(&root).expect("status"), [("Language".to_string(), want.to_string())]);
        std::fs::remove_dir_all(root).ok();
    }
}
