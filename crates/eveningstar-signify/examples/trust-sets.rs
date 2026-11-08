//! Writes the firmware trust sets, or the README's key table, from the key
//! list `nix/keys.nix` gives as JSON (the `keys` package).
//!
//! Usage: `trust-sets <keys.json> rust|readme`, from the repository's root,
//! which the key files' paths are relative to.
//!
//! Refuses a list with a gap in its generations, a key file that doesn't read,
//! a key `check_key` refuses, a keynum twice, generations without a signing
//! one, a signing generation that is lost, compromised or not listed, and
//! fewer than three usable generations from the signing one.

use core::fmt::Write as _;
use std::process::ExitCode;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use eveningstar_signify::{Keynum, check_key, public_key};
use serde_json::Value;

/// How many generations a release trusts: the signing one and the next two.
const TRUSTED_GENERATIONS: usize = 3;

/// A key of the list, read and checked.
struct Key {
    /// The release generation, or `None` for the development key.
    generation: Option<u64>,
    file: String,
    keynum: Keynum,
    public: [u8; 32],
    /// The public key line of the file, as signify prints it.
    line: String,
    lost: bool,
    compromised: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [json, what] = args.as_slice() else {
        eprintln!("usage: trust-sets <keys.json> rust|readme");
        return ExitCode::FAILURE;
    };
    let result = std::fs::read_to_string(json)
        .map_err(|error| format!("{json}: {error}"))
        .and_then(|text| serde_json::from_str(&text).map_err(|error| format!("{json}: {error}")))
        .and_then(|list: Value| read(&list))
        .and_then(|(keys, signing)| match what.as_str() {
            "rust" => Ok(rust(&keys, signing)),
            "readme" => Ok(readme(&keys, signing)),
            _ => Err(format!("unknown output {what}: rust or readme")),
        });
    match result {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("trust-sets: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The keys of `list`, the development key first and then the generations in
/// order, and the signing generation, once every rule holds.
fn read(list: &Value) -> Result<(Vec<Key>, Option<u64>), String> {
    let dev = list["dev"].as_str().ok_or("dev isn't a file")?;
    let mut keys = vec![key(None, dev, false, false)?];
    let mut generations = list["generations"]
        .as_array()
        .ok_or("generations isn't a list")?
        .iter()
        .map(|entry| {
            let generation = entry["generation"]
                .as_u64()
                .ok_or("a generation isn't a number")?;
            let file = entry["file"].as_str().ok_or("a generation has no file")?;
            let flag = |name: &str| entry[name].as_bool().unwrap_or(false);
            key(Some(generation), file, flag("lost"), flag("compromised"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    generations.sort_by_key(|key| key.generation);
    for (index, key) in generations.iter().enumerate() {
        if key.generation != Some(index as u64 + 1) {
            return Err(format!(
                "generation {} follows a gap: generations are numbered from 1",
                key.generation.unwrap_or(0)
            ));
        }
    }
    keys.extend(generations);
    for (index, key) in keys.iter().enumerate() {
        if let Some(other) = keys[index + 1..]
            .iter()
            .find(|other| other.keynum == key.keynum)
        {
            return Err(format!("{} and {} share a keynum", key.file, other.file));
        }
    }
    let signing = list["signing"].as_u64();
    match signing {
        None if keys.len() > 1 => Err("generations are listed, but none is signing".to_owned()),
        None => Ok((keys, None)),
        Some(signing) => {
            let current = keys
                .iter()
                .find(|key| key.generation == Some(signing))
                .ok_or(format!("the signing generation {signing} isn't listed"))?;
            if current.lost || current.compromised {
                return Err(format!(
                    "the signing generation {signing} is lost or compromised"
                ));
            }
            let trusted = trusted(&keys, signing).count();
            if trusted < TRUSTED_GENERATIONS {
                return Err(format!(
                    "{trusted} usable generations from {signing}: a release trusts {TRUSTED_GENERATIONS}"
                ));
            }
            Ok((keys, Some(signing)))
        }
    }
}

/// The key in `file`, read with [`public_key`] and checked with
/// [`check_key`].
fn key(generation: Option<u64>, file: &str, lost: bool, compromised: bool) -> Result<Key, String> {
    let text = std::fs::read(file).map_err(|error| format!("{file}: {error}"))?;
    let (keynum, key) = public_key(&text).map_err(|error| format!("{file}: {error}"))?;
    check_key(&key).map_err(|error| format!("{file}: {error}"))?;
    let mut decoded = b"Ed".to_vec();
    decoded.extend_from_slice(&keynum);
    decoded.extend_from_slice(&key);
    Ok(Key {
        generation,
        file: file.to_owned(),
        keynum,
        public: key,
        line: STANDARD.encode(decoded),
        lost,
        compromised,
    })
}

/// The generations a release signed with `signing` trusts: it and the next
/// usable ones, three in all.
fn trusted(keys: &[Key], signing: u64) -> impl Iterator<Item = &Key> {
    keys.iter()
        .filter(move |key| {
            key.generation
                .is_some_and(|generation| generation >= signing)
        })
        .filter(|key| !key.lost && !key.compromised)
        .take(TRUSTED_GENERATIONS)
}

/// The trust set module.
fn rust(keys: &[Key], signing: Option<u64>) -> String {
    let mut out = String::from(
        "//! Firmware trust sets, generated from `nix/keys.nix` by the `trust-sets`\n\
         //! command and not edited by hand.\n\
         \n\
         use crate::key::{Generation, Purpose, TrustSet, TrustedKey};\n",
    );
    let release: Vec<&Key> = signing
        .map(|signing| trusted(keys, signing).collect())
        .unwrap_or_default();
    for key in keys.iter().filter(|key| {
        key.generation.is_none() || release.iter().any(|r| r.generation == key.generation)
    }) {
        let (name, doc, generation) = match key.generation {
            None => (
                "DEVELOPMENT_KEY".to_owned(),
                format!(
                    "The development key, `{}`, whose secret key is public.",
                    key.file
                ),
                "Generation::Development".to_owned(),
            ),
            Some(n) => (
                format!("RELEASE_{n}"),
                format!("Release generation {n}, `{}`.", key.file),
                format!("Generation::Release({n})"),
            ),
        };
        let visibility = if key.generation.is_none() { "pub " } else { "" };
        let _ = write!(
            out,
            "\n{}{visibility}const {name}: TrustedKey = TrustedKey::new(\n\
             Purpose::Firmware,\n{generation},\n{},\n{},\n);\n",
            doc_comment(&doc),
            bytes(&key.keynum),
            bytes(&key.public),
        );
    }
    let names: Vec<String> = release
        .iter()
        .map(|key| format!("RELEASE_{}", key.generation.unwrap_or(0)))
        .collect();
    let generations = release
        .iter()
        .map(|key| key.generation.unwrap_or(0).to_string())
        .collect::<Vec<_>>();
    let development_doc = if release.is_empty() {
        "the development key alone, as no release generation exists yet.".to_owned()
    } else {
        format!(
            "the development key and generations {}.",
            list(&generations)
        )
    };
    let _ = write!(
        out,
        "\n{}pub const DEVELOPMENT: TrustSet = TrustSet::new(Purpose::Firmware, &[DEVELOPMENT_KEY{}]);\n",
        doc_comment(&format!(
            "What a development build trusts: {development_doc}"
        )),
        names.iter().fold(String::new(), |mut out, name| {
            let _ = write!(out, ", {name}");
            out
        }),
    );
    if !release.is_empty() {
        let _ = write!(
            out,
            "\n{}pub const RELEASE: TrustSet = TrustSet::new(Purpose::Firmware, &[{}]);\n",
            doc_comment(&format!(
                "What a release build trusts: generations {}.",
                list(&generations)
            )),
            names.join(", "),
        );
    }
    out
}

/// The README's key table, as aligned Markdown.
fn readme(keys: &[Key], signing: Option<u64>) -> String {
    let release: Vec<Option<u64>> = signing
        .map(|signing| trusted(keys, signing).map(|key| key.generation).collect())
        .unwrap_or_default();
    let rows: Vec<[String; 3]> = keys
        .iter()
        .map(|key| {
            let (name, status) = match key.generation {
                None => ("development".to_owned(), "trusted by development builds"),
                Some(n) => (
                    n.to_string(),
                    if key.compromised {
                        "compromised, never trusted"
                    } else if key.lost {
                        "lost, never trusted"
                    } else if Some(n) == signing {
                        "signs releases"
                    } else if release.contains(&key.generation) {
                        "trusted, signs a later release"
                    } else if signing.is_some_and(|signing| n < signing) {
                        "retired"
                    } else {
                        "not trusted yet"
                    },
                ),
            };
            [name, status.to_owned(), format!("`{}`", key.line)]
        })
        .collect();
    let header = [
        "Key".to_owned(),
        "Status".to_owned(),
        "Public key".to_owned(),
    ];
    let widths: Vec<usize> = (0..3)
        .map(|column| {
            rows.iter()
                .chain([&header])
                .map(|row| row[column].len())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String; 3]| {
        let cells: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:width$}"))
            .collect();
        format!("| {} |\n", cells.join(" | "))
    };
    let mut out = line(&header);
    let dashes: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    let _ = writeln!(out, "| {} |", dashes.join(" | "));
    for row in &rows {
        out.push_str(&line(row));
    }
    out
}

/// `bytes` as a Rust array of hex literals.
fn bytes(bytes: &[u8]) -> String {
    let items: Vec<String> = bytes.iter().map(|byte| format!("0x{byte:02x}")).collect();
    format!("[{}]", items.join(", "))
}

/// `items` as an English list: `a`, `a and b`, `a, b and c`.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `text` as a doc comment, wrapped at 80 columns.
fn doc_comment(text: &str) -> String {
    let mut out = String::new();
    let mut line = String::from("///");
    for word in text.split(' ') {
        if line.len() + 1 + word.len() > 80 && line.len() > 3 {
            out.push_str(&line);
            out.push('\n');
            line = String::from("///");
        }
        line.push(' ');
        line.push_str(word);
    }
    out.push_str(&line);
    out.push('\n');
    out
}
