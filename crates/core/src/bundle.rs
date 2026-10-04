//! Uploads contain data, never archive entries, symlinks, build commands or host paths.
use crate::{MAX_CONFIG_BYTES, MAX_DECODED_BYTES, MAX_STAGING_DIRECTORIES, MAX_UPLOAD_FILES};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{
    de::{self, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::{collections::BTreeSet, fmt, io::Write};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadFile {
    #[serde(deserialize_with = "deserialize_path")]
    pub path: String,
    #[serde(deserialize_with = "deserialize_content")]
    pub content: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedBundle {
    #[serde(deserialize_with = "deserialize_config")]
    pub config: Value,
    #[serde(deserialize_with = "deserialize_files")]
    pub modules: Vec<UploadFile>,
    #[serde(deserialize_with = "deserialize_files")]
    pub assets: Vec<UploadFile>,
}
fn deserialize_config<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    // Raw JSON has bounded memory even for millions of tiny array/object entries;
    // reject its size before materializing a potentially much larger Value tree.
    let raw = Box::<serde_json::value::RawValue>::deserialize(d)?;
    if raw.get().len() > MAX_CONFIG_BYTES {
        return Err(de::Error::custom("config exceeds 64 KiB"));
    }
    serde_json::from_str(raw.get()).map_err(de::Error::custom)
}
fn bounded_string<'de, D: Deserializer<'de>>(d: D, limit: usize) -> Result<String, D::Error> {
    struct Bounded(usize);
    impl Visitor<'_> for Bounded {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "a string of at most {} bytes", self.0)
        }
        fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
            if value.len() > self.0 {
                Err(E::custom("upload string exceeds limit"))
            } else {
                Ok(value.to_owned())
            }
        }
        fn visit_string<E: de::Error>(self, value: String) -> Result<String, E> {
            if value.len() > self.0 {
                Err(E::custom("upload string exceeds limit"))
            } else {
                Ok(value)
            }
        }
    }
    d.deserialize_string(Bounded(limit))
}
fn deserialize_path<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_string(d, 512)
}
fn deserialize_content<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_string(d, MAX_DECODED_BYTES.div_ceil(3) * 4)
}
fn deserialize_files<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<UploadFile>, D::Error> {
    struct Files;
    // Reject before allocating even the first field of the 4097th entry.
    struct Excess;
    impl<'de> Deserialize<'de> for Excess {
        fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
            Err(de::Error::custom("upload exceeds 4096 files"))
        }
    }
    impl<'de> Visitor<'de> for Files {
        type Value = Vec<UploadFile>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "at most 4096 upload files")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut files = Vec::new();
            loop {
                if files.len() == MAX_UPLOAD_FILES {
                    let _ = seq.next_element::<Excess>()?;
                    break;
                }
                match seq.next_element()? {
                    Some(file) => files.push(file),
                    None => break,
                }
            }
            Ok(files)
        }
    }
    d.deserialize_seq(Files)
}
struct ConfigSize(usize);
impl Write for ConfigSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        if self.0 > MAX_CONFIG_BYTES {
            return Err(std::io::Error::other("config exceeds 64 KiB"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn bounded_config(value: &Value) -> Result<(), String> {
    serde_json::to_writer(ConfigSize(0), value).map_err(|_| "config exceeds 64 KiB".into())
}

pub fn valid_upload_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && part.len() <= 255)
        && !path.starts_with('/')
        && !path.contains(':')
}
impl PreparedBundle {
    /// Validate before cloning and normalize only staging/build paths. All other
    /// binding/asset options stay authoritative to the pinned native parser.
    pub fn normalize(&self) -> Result<Self, String> {
        if self.modules.len().saturating_add(self.assets.len()) > MAX_UPLOAD_FILES {
            return Err("bundle exceeds 4096 files".into());
        }
        bounded_config(&self.config)?;
        let object = self
            .config
            .as_object()
            .ok_or("bundle config must be an object")?;
        if object.contains_key("containers") {
            return Err("container deployments are not supported by the SSH publisher".into());
        }
        if object.contains_key("python")
            || object.contains_key("python_runtime")
            || object
                .get("compatibility_flags")
                .and_then(Value::as_array)
                .is_some_and(|flags| {
                    flags.iter().any(|flag| {
                        flag.as_str()
                            .is_some_and(|s| s.to_ascii_lowercase().contains("python"))
                    })
                })
        {
            return Err("Python deployments are not supported by the SSH publisher".into());
        }
        let has_main = object.contains_key("main");
        if let Some(main) = object.get("main") {
            let main = main.as_str().ok_or("config main must be a string")?;
            if ![".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx"]
                .iter()
                .any(|ext| main.ends_with(ext))
            {
                return Err(
                    "SSH publish supports only JavaScript/TypeScript Worker entry points".into(),
                );
            }
        } else {
            if object
                .get("no_bundle")
                .is_some_and(|value| value != &Value::Bool(false))
            {
                return Err("no_bundle without main is invalid".into());
            }
            if !self.modules.is_empty() {
                return Err("asset-only bundles cannot include Worker modules".into());
            }
        }
        let has_assets = object.contains_key("assets");
        if let Some(assets) = object.get("assets") {
            if !assets.is_object() {
                return Err("config assets must be an object".into());
            }
        } else if !self.assets.is_empty() {
            return Err("asset files require an assets configuration".into());
        }
        if !has_main && !has_assets {
            return Err("bundle config needs main or assets".into());
        }
        let mut total = 0usize;
        // Bound inode/directory work before any materialization. Counting only
        // files would allow thousands of individually short, deeply nested paths
        // to create hundreds of thousands of root-owned directories.
        let mut directories = BTreeSet::from(["modules".to_owned(), "assets".to_owned()]);
        for (kind, files) in [("modules", &self.modules), ("assets", &self.assets)] {
            let mut paths = BTreeSet::new();
            for file in files {
                if !valid_upload_path(&file.path) {
                    return Err("invalid upload path".into());
                }
                if kind == "modules"
                    && file.path != "index.js"
                    && !file.path.ends_with(".wasm")
                    && !file.path.ends_with(".wasm?module")
                {
                    return Err("modules must be index.js or precompiled .wasm files".into());
                }
                if !paths.insert(file.path.as_str()) {
                    return Err("duplicate upload path".into());
                }
                for (at, _) in file.path.match_indices('/') {
                    directories.insert(format!("{kind}/{}", &file.path[..at]));
                    if directories.len() > MAX_STAGING_DIRECTORIES {
                        return Err("bundle exceeds 8192 staging directories".into());
                    }
                    if paths.contains(&file.path[..at]) {
                        return Err("upload file/directory path collision".into());
                    }
                }
                let prefix = format!("{}/", file.path);
                if paths
                    .range(prefix.as_str()..)
                    .next()
                    .is_some_and(|other| other.starts_with(&prefix))
                {
                    return Err("upload file/directory path collision".into());
                }
                if file.content.len() > MAX_DECODED_BYTES.div_ceil(3) * 4 {
                    return Err("upload exceeds decoded size limit".into());
                }
                let decoded = STANDARD
                    .decode(&file.content)
                    .map_err(|_| "invalid base64 upload content")?;
                total = total
                    .checked_add(decoded.len())
                    .ok_or("upload size overflow")?;
                if total > MAX_DECODED_BYTES {
                    return Err("bundle exceeds 24 MiB decoded files".into());
                }
            }
            if kind == "modules" && has_main && !paths.contains("index.js") {
                return Err("Worker bundle needs modules/index.js".into());
            }
        }
        let mut output = self.clone();
        let object = output.config.as_object_mut().expect("validated object");
        if has_main {
            object.insert("main".into(), Value::String("modules/index.js".into()));
            object.insert("no_bundle".into(), Value::Bool(true));
        } else {
            object.remove("no_bundle");
        }
        object.remove("define");
        object.remove("rules");
        if let Some(assets) = object.get_mut("assets") {
            assets
                .as_object_mut()
                .expect("validated object")
                .insert("directory".into(), Value::String("assets".into()));
        }
        bounded_config(&output.config)?;
        Ok(output)
    }
}
