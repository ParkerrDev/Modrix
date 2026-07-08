// SPDX-License-Identifier: GPL-2.0-only
//! Parse the FOMOD installers of staged mod trees passed as arguments and
//! print what the default (non-interactive) install would do. Development
//! smoke tool: `cargo run -p modman-plugin --example fomod-smoke -- <trees>`.

#[expect(
    clippy::print_stdout,
    reason = "a development smoke tool; stdout is its interface"
)]
fn main() {
    let mut parsed: usize = 0;
    let mut failed: usize = 0;
    for arg in std::env::args().skip(1) {
        let root = std::path::PathBuf::from(&arg);
        let label = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match modman_plugin::fomod::parse(&root) {
            Ok(Some(installer)) => {
                let selections = modman_plugin::fomod::defaults(&installer);
                let ops = modman_plugin::fomod::resolve(&installer, &selections);
                let flags = modman_plugin::fomod::flags_of(&installer, &selections);
                let visible = installer
                    .steps
                    .iter()
                    .filter(|s| s.visible.as_ref().is_none_or(|d| d.eval(&flags)))
                    .count();
                println!(
                    "OK   {label}: steps={} visible={visible} ops={} name={:?} version={:?}",
                    installer.steps.len(),
                    ops.len(),
                    installer.info_name,
                    installer.info_version,
                );
                parsed = parsed.saturating_add(1);
            }
            Ok(None) => println!("NONE {label}"),
            Err(error) => {
                println!("FAIL {label}: {error}");
                failed = failed.saturating_add(1);
            }
        }
    }
    println!("{parsed} parsed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
