//! Install-identity code generation, shared by the two build scripts that need it.
//!
//! `platform/src/storage/state.rs` is compiled into BOTH the `plx_platform` library and the
//! `multiplex-storage` helper (the helper is its own workspace package and does not depend on the
//! application crates), and it
//! `include!`s the `Flavor` enum generated here from `ci/install-identities.json`. Each package's
//! `build.rs` pulls this file in with `#[path]` and calls [`emit`], so the schema cannot drift
//! between the client and the helper: a single generator, two callers, no copy.

use std::path::{Path, PathBuf};

/// Packaging owns the identities; generating the schema here makes adding an install update
/// the client and helper together, without a Python interpreter in the Rust build or on the TV.
pub fn emit(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    let identities: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path).expect("install identities")).unwrap();
    let mut variants = String::new();
    let mut apps = String::new();
    let mut objects = String::new();
    let mut legacies = String::new();
    let mut object_ids = std::collections::HashSet::new();
    let mut legacy_ids = Vec::new();
    for identity in identities {
        let name = identity["name"].as_str().expect("flavor name");
        assert!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase()));
        let variant = name[..1].to_ascii_uppercase() + &name[1..];
        let app = identity["app_id"].as_str().expect("app id");
        let object = identity["object_id"].as_str().expect("DB8 object id");
        // Device (webOS 4.10.2): 16 bytes = generated base64 ID;
        // 17+ bytes fail DB8 put with -3968 "Invalid _id length".
        assert!(
            (1..=15).contains(&object.len()),
            "invalid DB8 object_id {object:?} for flavor {name}: custom IDs must be 1..=15 bytes"
        );
        // DB8 IDs are global across kinds, so every install must use a distinct ID.
        assert!(
            object_ids.insert(object.to_owned()),
            "duplicate DB8 object_id {object:?} for flavor {name}"
        );
        // The id an install's record had before it was renamed, read once to carry the record
        // over (`storage_service::backend::Backend::adopt_legacy_record`) and never written. The
        // stable install's is `plxstate.stable`, which PlxNative (`com.beb.plxnative`) also uses,
        // so the record found there is adopted only when its kind is this install's own.
        let legacy = match identity.get("legacy_object_id") {
            None => None,
            Some(id) => {
                let id = id.as_str().expect("legacy DB8 object id");
                assert!((1..=15).contains(&id.len()), "invalid legacy DB8 object_id {id:?} for flavor {name}");
                legacy_ids.push(id.to_owned());
                Some(id.to_owned())
            }
        };
        legacies.push_str(&format!("Self::{variant} => {legacy:?},\n"));
        variants.push_str(&format!("{variant},\n"));
        apps.push_str(&format!("{app:?} => Some(Self::{variant}),\n"));
        objects.push_str(&format!("Self::{variant} => {object:?},\n"));
    }
    for legacy in &legacy_ids {
        assert!(!object_ids.contains(legacy), "legacy DB8 object_id {legacy:?} is a current one");
    }
    let generated = format!(
        "#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
         #[serde(rename_all = \"snake_case\")]
         pub enum Flavor {{ {variants} }}
         impl Flavor {{
             pub fn from_app_id(id: &str) -> Option<Self> {{
                 match id {{ {apps} _ => None }}
             }}
             pub fn object_id(self) -> &'static str {{ match self {{ {objects} }} }}
             pub fn legacy_object_id(self) -> Option<&'static str> {{ match self {{ {legacies} }} }}
         }}"
    );
    std::fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("install_identities.rs"),
        generated,
    )
    .unwrap();
}
