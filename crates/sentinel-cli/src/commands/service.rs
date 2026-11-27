//! Generation of OS-level service definitions for Sentinel.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::args::InstallServiceArgs;
use crate::config::load_manifest;

pub fn run_install_service(args: InstallServiceArgs) -> i32 {
    if args.systemd && args.launchd {
        eprintln!("Error: --systemd and --launchd are mutually exclusive");
        return 10;
    }

    let service_type = if args.systemd {
        "systemd"
    } else if args.launchd || env::consts::OS == "macos" {
        "launchd"
    } else if env::consts::OS == "linux" {
        "systemd"
    } else {
        eprintln!("Error: unsupported operating system; select --systemd or --launchd explicitly");
        return 10;
    };

    let manifest = match canonical_regular_file(&args.manifest) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Error: {error}");
            return 10;
        }
    };

    let rpc_env = match env::var("SOROBAN_RPC_URL") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => {
            eprintln!("Error: SOROBAN_RPC_URL must be UTF-8");
            return 10;
        }
    };
    if let Err(error) = load_manifest(&manifest, rpc_env.as_deref()) {
        eprintln!("Error: invalid manifest: {error}");
        return 10;
    }

    let binary = match env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
        .or_else(|| {
            env::var_os("PATH").and_then(|paths| {
                env::split_paths(&paths).find_map(|dir| {
                    let candidate = dir.join("sentinel");
                    candidate
                        .is_file()
                        .then(|| candidate.canonicalize().ok())
                        .flatten()
                })
            })
        }) {
        Some(path) => path,
        None => {
            eprintln!("Error: current Sentinel binary path could not be resolved");
            return 10;
        }
    };

    let rendered = if service_type == "systemd" {
        render_systemd(&binary, &manifest, args.user.as_deref())
    } else {
        render_launchd(&binary, &manifest)
    };

    if let Err(error) = validate_generated(service_type, &rendered) {
        eprintln!("Error: generated service is invalid: {error}");
        return 10;
    }

    let filename = if service_type == "systemd" {
        "sentinel.service"
    } else {
        "com.soroban.sentinel.plist"
    };

    if let Some(output_dir) = args.output_dir {
        if let Err(error) = write_service(&output_dir, filename, service_type, &rendered) {
            eprintln!("Error: {error}");
            return 10;
        }
        println!("{}", output_dir.join(filename).display());
    } else {
        print!("{rendered}");
    }
    0
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "manifest is not a regular file: {}",
            path.display()
        ));
    }
    path.canonicalize()
        .map_err(|error| format!("could not resolve manifest path: {error}"))
}

fn render_systemd(binary: &Path, manifest: &Path, user: Option<&str>) -> String {
    let user_line = user.map_or_else(String::new, |value| format!("User={value}\n"));
    format!(
        "[Unit]\nDescription=Soroban Sentinel autonomous TTL monitor\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\n{user_line}ExecStart={} watch --config {}\nRestart=on-failure\nRestartSec=30\nNoNewPrivileges=true\nProtectSystem=full\nPrivateTmp=true\nProtectHome=true\n\n[Install]\nWantedBy=multi-user.target\n",
        escape_systemd(binary), escape_systemd(manifest)
    )
}

fn render_launchd(binary: &Path, manifest: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>com.soroban.sentinel</string>\n  <key>ProgramArguments</key><array><string>{}</string><string>watch</string><string>--config</string><string>{}</string></array>\n  <key>KeepAlive</key><true/>\n  <key>RunAtLoad</key><true/>\n  <key>StandardOutPath</key><string>/tmp/soroban-sentinel.log</string>\n  <key>StandardErrorPath</key><string>/tmp/soroban-sentinel.err.log</string>\n  <key>EnvironmentVariables</key><dict><key>PATH</key><string>/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>\n</dict>\n</plist>\n",
        xml_escape(&binary.to_string_lossy()), xml_escape(&manifest.to_string_lossy())
    )
}

fn escape_systemd(path: &Path) -> String {
    path.to_string_lossy()
        .replace('%', "%%")
        .replace(' ', "\\x20")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn validate_generated(service_type: &str, rendered: &str) -> Result<(), String> {
    if rendered.is_empty() {
        return Err("empty output".to_owned());
    }
    if service_type == "systemd" {
        for required in [
            "[Unit]",
            "[Service]",
            "[Install]",
            "Restart=on-failure",
            "NoNewPrivileges=true",
            "ProtectSystem=full",
            "ExecStart=",
        ] {
            if !rendered.contains(required) {
                return Err(format!("missing {required}"));
            }
        }
    } else {
        for required in [
            "<plist",
            "<key>KeepAlive</key>",
            "<key>StandardOutPath</key>",
            "<key>EnvironmentVariables</key>",
        ] {
            if !rendered.contains(required) {
                return Err(format!("missing {required}"));
            }
        }
        if rendered.matches("<dict>").count() == 0 || !rendered.ends_with("</plist>\n") {
            return Err("invalid plist structure".to_owned());
        }
    }
    Ok(())
}

fn write_service(
    dir: &Path,
    filename: &str,
    _service_type: &str,
    rendered: &str,
) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|error| format!("could not create output directory: {error}"))?;
    let target = dir.join(filename);
    fs::write(&target, rendered)
        .map_err(|error| format!("could not write {}: {error}", target.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if _service_type == "systemd" {
            0o644
        } else {
            0o600
        };
        fs::set_permissions(&target, fs::Permissions::from_mode(mode)).map_err(|error| {
            format!("could not set permissions on {}: {error}", target.display())
        })?;
    }
    Ok(())
}
