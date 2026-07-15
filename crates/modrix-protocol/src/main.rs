// SPDX-License-Identifier: GPL-2.0-only
//! `nxm://` protocol forwarder.
//!
//! The OS launches this tiny binary for `nxm://` links. It reads the
//! single-instance lockfile, connects to the running Modrix instance over
//! the loopback IPC seam, and forwards the URL - then exits. It also registers
//! and unregisters itself as the OS handler for the `nxm` scheme.

use std::io::Write;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use modrix_core::Paths;
use modrix_ipc::secondary_from_lock;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut data_dir: Option<PathBuf> = None;
    let mut link: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return usage(),
            "--register" => return register::install(),
            "--unregister" => return register::uninstall(),
            "--data-dir" => {
                let dir = args.next().context("--data-dir needs a path")?;
                data_dir = Some(PathBuf::from(dir));
            }
            _ => link = Some(arg),
        }
    }
    let uri = link.context("usage: modrix-protocol <nxm://…> | --register | --unregister")?;
    forward(&uri, data_dir.as_deref()).await
}

/// Forward an `nxm://` URL to the running primary instance.
async fn forward(uri: &str, data_dir: Option<&Path>) -> Result<()> {
    let paths = match data_dir {
        Some(dir) => Paths::rooted_at(dir),
        None => Paths::resolve().context("resolving the Modrix data directory")?,
    };
    let secondary = secondary_from_lock(&paths.instance_lock()).context(
        "Modrix does not appear to be running - start it (or the background \
         service), then click the download link again",
    )?;
    let reply = secondary
        .send("/nxm", uri)
        .await
        .context("forwarding the link to Modrix")?;
    if reply.status == 200 {
        Ok(())
    } else {
        bail!(
            "Modrix rejected the link (HTTP {}): {}",
            reply.status,
            reply.body
        );
    }
}

fn usage() -> Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "modrix-protocol - forwards nxm:// links to Modrix\n\n\
         usage:\n  modrix-protocol <nxm://…>   forward a download link\n  \
         modrix-protocol --register     register as the OS nxm:// handler\n  \
         modrix-protocol --unregister   remove the registration"
    )?;
    Ok(())
}

/// Per-OS registration as the `nxm` scheme handler. Linux is automated here;
/// Windows and macOS print the manual steps (their APIs are validated by hand,
/// not in CI).
mod register {
    use anyhow::Result;

    pub fn install() -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            linux::install()
        }
        #[cfg(not(target_os = "linux"))]
        {
            manual_instructions()
        }
    }

    pub fn uninstall() -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            linux::uninstall()
        }
        #[cfg(not(target_os = "linux"))]
        {
            manual_instructions()
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn manual_instructions() -> Result<()> {
        use std::io::Write as _;
        let mut out = std::io::stdout().lock();
        writeln!(
            out,
            "Automatic nxm:// registration is implemented for Linux. On this OS, \
             register the `nxm` scheme to point at this executable:\n\
             - Windows: set HKCU\\Software\\Classes\\nxm (URL Protocol) + shell open \
             command to `\"<this exe>\" \"%1\"`.\n\
             - macOS: add CFBundleURLTypes (scheme `nxm`) to the app's Info.plist and \
             call LSSetDefaultHandlerForURLScheme."
        )?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use std::io::Write;
        use std::path::PathBuf;
        use std::process::Command;

        use anyhow::{Context, Result};

        const DESKTOP_FILE: &str = "modrix-nxm.desktop";
        const MIME: &str = "x-scheme-handler/nxm";

        pub fn install() -> Result<()> {
            let exe = std::env::current_exe().context("locating this executable")?;
            let apps = applications_dir()?;
            std::fs::create_dir_all(&apps)
                .with_context(|| format!("creating {}", apps.display()))?;
            let desktop = apps.join(DESKTOP_FILE);
            let contents = format!(
                "[Desktop Entry]\nType=Application\nName=Modrix nxm handler\n\
                 Exec={} %u\nNoDisplay=true\nMimeType={MIME};\n",
                exe.display()
            );
            std::fs::write(&desktop, contents)
                .with_context(|| format!("writing {}", desktop.display()))?;
            run("xdg-mime", &["default", DESKTOP_FILE, MIME])?;
            let _ = run("update-desktop-database", &[apps_str(&apps).as_str()]);
            report(&format!("registered nxm:// → {}", exe.display()))
        }

        pub fn uninstall() -> Result<()> {
            let desktop = applications_dir()?.join(DESKTOP_FILE);
            let _ = std::fs::remove_file(&desktop);
            report("unregistered nxm:// handler")
        }

        fn applications_dir() -> Result<PathBuf> {
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
                .context("neither XDG_DATA_HOME nor HOME is set")?;
            Ok(base.join("applications"))
        }

        fn apps_str(apps: &std::path::Path) -> String {
            apps.to_string_lossy().into_owned()
        }

        fn run(program: &str, args: &[&str]) -> Result<()> {
            let status = Command::new(program)
                .args(args)
                .status()
                .with_context(|| format!("running {program}"))?;
            anyhow::ensure!(status.success(), "{program} exited with {status}");
            Ok(())
        }

        fn report(message: &str) -> Result<()> {
            let mut out = std::io::stdout().lock();
            writeln!(out, "{message}")?;
            Ok(())
        }
    }
}
