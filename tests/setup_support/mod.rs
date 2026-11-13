//! A throwaway machine for the setup and doctor tests: a home directory, a project that is a
//! git work tree, and a `bin` directory that is all of `PATH` besides the system's own.
//! Included with `#[path = "setup_support/mod.rs"] mod sandbox;`.
//!
//! The environment of every command starts empty, so no variable of the developer's shell
//! (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_CONFIG_HOME`, ...) can point a test at real
//! configuration, and no harness installed on the developer's machine is on `PATH` unless a
//! test links it in on purpose.
#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const BIN: &str = env!("CARGO_BIN_EXE_codefast");

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct Sandbox {
    dir: PathBuf,
    pub home: PathBuf,
    pub project: PathBuf,
    pub bin: PathBuf,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    /// The command succeeded and said nothing on stderr; returns what it printed.
    pub fn ok(&self) -> &str {
        assert_eq!((self.code, self.stderr.as_str()), (0, ""), "stdout: {}", self.stdout);
        &self.stdout
    }
}

pub fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A JSON file, parsed strictly: what setup writes into a fresh file is plain JSON.
pub fn json(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

impl Sandbox {
    /// A fresh sandbox under the build's own temporary directory. The project carries a `.git`
    /// directory, so it is the repository root whatever lies above it.
    pub fn new(tag: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("setup-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["home", "project/.git", "bin"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let dir = dir.canonicalize().unwrap();
        Sandbox { home: dir.join("home"), project: dir.join("project"), bin: dir.join("bin"), dir }
    }

    pub fn home(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }

    pub fn project(&self, rel: &str) -> PathBuf {
        self.project.join(rel)
    }

    /// `codefast ARGS` in the project, with only the sandbox in its environment.
    pub fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> Out {
        self.run_in(&self.project, args, envs)
    }

    /// `codefast ARGS` in `dir`: a directory below the project's top, or another project.
    pub fn run_in(&self, dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Out {
        let mut c = Command::new(BIN);
        c.env_clear().env("HOME", &self.home).env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()));
        for (k, v) in envs {
            c.env(k, v);
        }
        let out = c.current_dir(dir).args(args).stdin(Stdio::null()).output().expect("run codefast");
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    pub fn setup(&self, args: &[&str]) -> Out {
        self.setup_env(args, &[])
    }

    pub fn setup_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Out {
        let mut all = vec!["setup"];
        all.extend(args);
        self.run(&all, envs)
    }

    /// Put an executable called `name` on the sandbox's `PATH`. It appends its arguments to a
    /// log (see [`Sandbox::calls`]) and then runs `body`, a piece of `sh`.
    pub fn fake(&self, name: &str, body: &str) {
        let log = self.dir.join(format!("{name}.calls"));
        let script = format!("#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{0}'; done\nprintf '%s\\n' '--end--' >> '{0}'\n{body}\n", log.display());
        let path = self.bin.join(name);
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// The argument vectors a fake was called with, in order.
    pub fn calls(&self, name: &str) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.dir.join(format!("{name}.calls"))).unwrap_or_default();
        let mut calls = Vec::new();
        let mut current = Vec::new();
        for line in text.lines() {
            if line == "--end--" {
                calls.push(std::mem::take(&mut current));
            } else {
                current.push(line.to_string());
            }
        }
        calls
    }

    /// Link a program installed on the developer's machine into the sandbox's `PATH`. False
    /// when it is not installed: the test that wanted it then has nothing to check.
    pub fn real(&self, name: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else { return false };
        let Some(found) = std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file()) else { return false };
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&found, self.bin.join(name)).is_ok()
        }
        #[cfg(not(unix))]
        {
            let _ = found;
            false
        }
    }

    /// Every file and directory in the sandbox with its size and modification time: equal
    /// snapshots mean nothing was created, removed or rewritten in between.
    pub fn snapshot(&self) -> Vec<(String, u64, std::time::SystemTime)> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, u64, std::time::SystemTime)>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(md) = std::fs::symlink_metadata(&path) else { continue };
                let size = if md.is_dir() { 0 } else { md.len() };
                out.push((path.strip_prefix(base).unwrap().display().to_string(), size, md.modified().unwrap()));
                if md.is_dir() {
                    walk(&path, base, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.dir, &self.dir, &mut out);
        out.sort();
        out
    }
}
