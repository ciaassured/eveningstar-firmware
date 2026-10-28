#![expect(
    clippy::large_stack_frames,
    reason = "the limit is the chip's, and this runs on the build machine"
)]

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    linker_be_nice();
    println!("cargo:rustc-link-arg=-Tdefmt.x");
    // make sure linkall.x is the last linker script (otherwise might cause problems with flip-link)
    println!("cargo:rustc-link-arg=-Tlinkall.x");
    web_ui();
}

/// The page the firmware serves, and what is sent with it.
struct WebUi {
    page: PathBuf,
    gzip: bool,
    etag: String,
    policy: String,
}

/// Generates `$OUT_DIR/web_ui.rs`, describing the page the firmware serves.
///
/// `EVENINGSTAR_WEB_UI` names the directory of the bundle built by Nix
/// (`web-ui-bundle`). Without it, the page says how to build the bundle.
fn web_ui() {
    println!("cargo:rerun-if-env-changed=EVENINGSTAR_WEB_UI");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));

    let web_ui = if let Some(dir) = std::env::var_os("EVENINGSTAR_WEB_UI") {
        bundle(&std::path::absolute(dir).expect("EVENINGSTAR_WEB_UI is not a valid path"))
    } else {
        placeholder(&out_dir)
    };

    let generated = format!(
        "pub(super) static PAGE: &[u8] = include_bytes!({page:?});\n\
         pub(super) const GZIP: bool = {gzip};\n\
         pub(super) const ETAG: &str = {etag:?};\n\
         pub(super) const CONTENT_SECURITY_POLICY: &str = {policy:?};\n",
        page = web_ui.page.display().to_string(),
        gzip = web_ui.gzip,
        etag = web_ui.etag,
        policy = web_ui.policy,
    );
    fs::write(out_dir.join("web_ui.rs"), generated).expect("write web_ui.rs");
}

/// The bundle in `dir`.
fn bundle(dir: &Path) -> WebUi {
    let page = dir.join("index.html.gz");
    let policy = dir.join("content-security-policy");
    let etag = dir.join("etag");
    for file in [&page, &policy, &etag] {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    assert!(page.is_file(), "cannot find {}", page.display());
    let etag = read_line(&etag);
    assert!(
        etag.len() > 2 && etag.starts_with('"') && etag.ends_with('"'),
        "the ETag in {} is not a quoted string: {etag}",
        dir.display(),
    );
    WebUi {
        page,
        gzip: true,
        etag,
        policy: read_line(&policy),
    }
}

/// A page saying the firmware was built without the web UI.
fn placeholder(out_dir: &Path) -> WebUi {
    println!(
        "cargo:warning=EVENINGSTAR_WEB_UI is not set: the device serves a placeholder instead of the web UI; \
         build it with `nix build .#web-ui-bundle` and set EVENINGSTAR_WEB_UI to the result"
    );
    let page = out_dir.join("web_ui_placeholder.html");
    fs::write(&page, PLACEHOLDER).expect("write the placeholder page");
    WebUi {
        page,
        gzip: false,
        etag: String::from("\"placeholder\""),
        policy: String::from("default-src 'none'"),
    }
}

const PLACEHOLDER: &str = "<!doctype html>\n\
<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Eveningstar</title></head>\n\
<body><h1>Eveningstar</h1>\n\
<p>This firmware was built without the web UI.</p>\n\
<p>Build it with <code>nix build .#web-ui-bundle</code> and set \
<code>EVENINGSTAR_WEB_UI</code> to the result when building the firmware.</p>\n\
</body></html>\n";

fn read_line(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .trim()
        .to_owned()
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                what if what.starts_with("_defmt_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler enabled. Make sure you have initialized `esp-rtos` or provided an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!(
                        "💡 Did you forget the `esp-alloc` dependency or didn't enable the `compat` feature on it?"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
