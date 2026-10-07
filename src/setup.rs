//! `buzz-backend-docker setup [host]`: the one-time step on the Desktop
//! machine. Everything else is configured in Buzz Desktop.

use crate::config::validate_host;
use crate::docker::DockerCli;
use std::path::{Path, PathBuf};

const NAME: &str = "buzz-backend-docker";

pub const USAGE: &str = "\
buzz-backend-docker — run Buzz agents on a Docker host

Buzz Desktop runs this program itself; you only need it once, to set up:

  buzz-backend-docker setup [HOST]

  Makes the provider visible to Buzz Desktop (links it into ~/.local/bin)
  and, given HOST (e.g. ssh://you@your-server), checks that Docker on that
  server answers.

  buzz-backend-docker --version";

pub fn run(host: Option<&str>) -> i32 {
    match setup(host) {
        Ok(report) => {
            println!("{report}");
            0
        }
        Err(e) => {
            eprintln!("setup failed: {e}");
            1
        }
    }
}

fn setup(host: Option<&str>) -> Result<String, String> {
    if let Some(h) = host {
        validate_host(h)?;
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate this program: {e}"))?;
    let target = stable_path(&exe);
    let link = Path::new(&home).join(".local/bin").join(NAME);
    let mut report = vec![link_into_place(&target, &link)?];

    let docker = DockerCli {
        host: host.map(str::to_string),
        context: None,
    };
    match (host, docker.server_version()) {
        (Some(h), Ok(v)) => report.push(format!("✓ Docker {v} answers at {h}")),
        (None, Ok(v)) => report.push(format!("✓ Docker {v} answers on this machine")),
        (Some(h), Err(e)) => return Err(format!("{}\n✗ Docker at {h} did not answer: {e}", report.join("\n"))),
        (None, Err(e)) => report.push(format!("! Docker on this machine did not answer ({e}); pass your server, e.g. setup ssh://you@your-server")),
    }

    report.push(String::new());
    report.push("In Buzz Desktop: open an agent's settings, choose the \"docker\" backend,".into());
    match host {
        Some(h) => report.push(format!("set Docker host to {h}, and press Start.")),
        None => report.push(
            "set Docker host to your server (ssh://you@your-server), and press Start.".into(),
        ),
    }
    Ok(report.join("\n"))
}

/// Homebrew runs us from a versioned Cellar path; link the stable
/// `<prefix>/bin` path instead so `brew upgrade` does not break the link.
fn stable_path(exe: &Path) -> PathBuf {
    let text = exe.to_string_lossy();
    match text.find("/Cellar/") {
        Some(i) => PathBuf::from(format!("{}/bin/{NAME}", &text[..i])),
        None => exe.to_path_buf(),
    }
}

fn link_into_place(target: &Path, link: &Path) -> Result<String, String> {
    if link == target {
        return Ok(format!(
            "✓ {} is already where Buzz Desktop looks",
            link.display()
        ));
    }
    match std::fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(link)
                .map_err(|e| format!("cannot replace {}: {e}", link.display()))?;
        }
        Ok(_) => {
            return Err(format!(
                "{} exists and is not a link; remove it and run setup again",
                link.display()
            ))
        }
        Err(_) => {}
    }
    let dir = link.parent().ok_or("invalid link path")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    symlink(target, link).map_err(|e| format!("cannot link {}: {e}", link.display()))?;
    Ok(format!(
        "✓ Linked {} → {}",
        link.display(),
        target.display()
    ))
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::fs::copy(target, link).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cellar_paths_map_to_the_stable_bin_path() {
        assert_eq!(
            stable_path(Path::new(
                "/opt/homebrew/Cellar/buzz-backend-docker/0.2.0/bin/buzz-backend-docker"
            )),
            PathBuf::from("/opt/homebrew/bin/buzz-backend-docker")
        );
        assert_eq!(
            stable_path(Path::new("/usr/local/bin/buzz-backend-docker")),
            PathBuf::from("/usr/local/bin/buzz-backend-docker")
        );
    }

    #[test]
    fn links_are_created_replaced_and_never_clobber_files() {
        let dir = std::env::temp_dir().join(format!("bbd-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("real");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&target, "x").unwrap();
        let link = dir.join("bin").join(NAME);

        link_into_place(&target, &link).unwrap();
        link_into_place(&target, &link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), target);

        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "user file").unwrap();
        assert!(link_into_place(&target, &link).is_err());
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "user file");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
