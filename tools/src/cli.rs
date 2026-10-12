// SPDX-License-Identifier: MIT
//! The command line: one command per former maintainer script, with the
//! options those scripts had.
use crate::{Context, build, proton, release, smoke, util};
use clap::{Parser, Subcommand};
use fenix_patch::{Error, Result, fsx, py};
use std::{ffi::OsString, path::PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "fenix-tools",
    about = "Maintainer tooling of the Fenix A320 Linux Patch",
    args_override_self = true,
    infer_long_args = true
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Rebuild the Wine modules from the exact archived sources and patches
    Build {
        #[arg(long)]
        prepare_only: bool,
        /// Fresh source/object directory [default: build in the repository]
        #[arg(long, value_parser = path)]
        build_dir: Option<PathBuf>,
        /// Update payload hashes after rebuilding; review and repin consumers before publishing
        #[arg(long)]
        record_build: bool,
        #[arg(long, value_parser = integer, default_value_t = jobs(), allow_negative_numbers = true)]
        jobs: i64,
    },
    /// Rebuild a Fenix overlay against one exact, pinned Proton Wine revision
    BuildProton {
        variant: String,
        #[arg(long, value_parser = path)]
        build_dir: PathBuf,
        #[arg(long)]
        prepare_only: bool,
        #[arg(long)]
        record_build: bool,
        #[arg(long, value_parser = integer, default_value_t = jobs(), allow_negative_numbers = true)]
        jobs: i64,
    },
    /// Run own-code probes against the release binaries in a fresh disposable prefix
    Smoke {
        #[arg(long, value_parser = path)]
        runner: PathBuf,
        /// SteamLinuxRuntime directory the Proton requires; also starts the runner inside its container
        #[arg(long, value_parser = path)]
        steam_runtime: Option<PathBuf>,
        /// Exact Proton overlay from bundle.json; omitted for Xodus
        #[arg(long)]
        variant: Option<String>,
        /// [default: build/smoke in the repository]
        #[arg(long, value_parser = path)]
        work: Option<PathBuf>,
        /// [default: build/downloads in the repository]
        #[arg(long, value_parser = path)]
        cache: Option<PathBuf>,
    },
    /// Allowlisted binary ZIP and complete corresponding source; no prefix traversal
    Release {
        /// The prebuilt, statically linked fenix-patch of this version
        #[arg(long, value_parser = path)]
        installer: PathBuf,
        /// Where the archives are written [default: dist in the repository]
        #[arg(long, value_parser = path)]
        output: Option<PathBuf>,
    },
}

/// argparse's `type=Path`.
fn path(text: &str) -> std::result::Result<PathBuf, String> {
    Ok(fsx::path(text))
}

/// argparse's `type=int`.
fn integer(text: &str) -> std::result::Result<i64, String> {
    py::int(text).map_err(|error| error.to_string())
}

/// `min(os.cpu_count() or 2, 8)`.
fn jobs() -> i64 {
    util::cpu_count().unwrap_or(2).min(8) as i64
}

fn execute(ctx: &Context<'_>, command: Command) -> Result<()> {
    match command {
        Command::Build {
            prepare_only,
            build_dir,
            record_build,
            jobs,
        } => build::run(
            ctx,
            &build::Options {
                prepare_only,
                build_dir,
                record_build,
                jobs,
            },
        ),
        Command::BuildProton {
            variant,
            build_dir,
            prepare_only,
            record_build,
            jobs,
        } => proton::run(
            ctx,
            &proton::Options {
                variant,
                build_dir,
                prepare_only,
                record_build,
                jobs,
            },
        ),
        Command::Smoke {
            runner,
            steam_runtime,
            variant,
            work,
            cache,
        } => smoke::run(
            ctx,
            &smoke::Options {
                runner,
                steam_runtime,
                variant,
                work,
                cache,
            },
        ),
        Command::Release { installer, output } => {
            release::run(ctx, &release::Options { installer, output })
        }
    }
}

/// Run one command line; the result is the process exit status.
pub fn run(ctx: &Context<'_>, argv: impl IntoIterator<Item = OsString>) -> i32 {
    let args = match Args::try_parse_from(argv) {
        Ok(args) => args,
        Err(error) => {
            let rendered = error.render().to_string();
            if error.use_stderr() {
                ctx.warn(rendered.trim_end_matches('\n'));
            } else {
                let _ = ctx.print(rendered.trim_end_matches('\n'));
            }
            return error.exit_code();
        }
    };
    match execute(ctx, args.command) {
        Ok(()) => 0,
        // Python ended these with a traceback and its last line, the message.
        Err(Error::Interrupted) => 130,
        Err(error) => {
            ctx.warn(&format!("fenix-tools: {error}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use clap::error::ErrorKind;

    fn parse(arguments: &[&str]) -> std::result::Result<Command, ErrorKind> {
        Args::try_parse_from(std::iter::once("fenix-tools").chain(arguments.iter().copied()))
            .map(|args| args.command)
            .map_err(|error| error.kind())
    }

    #[test]
    fn options_and_defaults_are_those_of_the_scripts() {
        let Command::Build {
            prepare_only,
            build_dir,
            record_build,
            jobs,
        } = parse(&["build"]).unwrap()
        else {
            panic!("build");
        };
        assert!(!prepare_only && !record_build && build_dir.is_none());
        // min(os.cpu_count() or 2, 8)
        assert!((1..=8).contains(&jobs), "{jobs}");
        assert_eq!(jobs, util::cpu_count().unwrap_or(2).min(8) as i64);

        let Command::Build {
            prepare_only,
            build_dir,
            record_build,
            jobs,
        } = parse(&[
            "build",
            "--record-build",
            "--jobs=1_6",
            "--build-dir",
            "out//x/",
            "--prepare-only",
        ])
        .unwrap()
        else {
            panic!("build");
        };
        assert!(prepare_only && record_build);
        // pathlib and int() read these as Python did.
        assert_eq!((build_dir, jobs), (Some(PathBuf::from("out/x")), 16));
        assert!(matches!(
            parse(&["build", "--jobs", "-2"]),
            Ok(Command::Build { jobs: -2, .. })
        ));

        let Command::BuildProton {
            variant,
            build_dir,
            prepare_only,
            record_build,
            jobs,
        } = parse(&[
            "build-proton",
            "--build-dir",
            "/b",
            "cachyos-11",
            "--jobs",
            "3",
        ])
        .unwrap()
        else {
            panic!("build-proton");
        };
        assert_eq!(
            (
                variant.as_str(),
                build_dir,
                prepare_only,
                record_build,
                jobs
            ),
            ("cachyos-11", PathBuf::from("/b"), false, false, 3)
        );

        let Command::Smoke {
            runner,
            steam_runtime,
            variant,
            work,
            cache,
        } = parse(&["smoke", "--runner", "/r"]).unwrap()
        else {
            panic!("smoke");
        };
        assert_eq!(runner, PathBuf::from("/r"));
        assert!(steam_runtime.is_none() && variant.is_none() && work.is_none() && cache.is_none());
        assert!(matches!(
            parse(&[
                "smoke",
                "--runner",
                "/r",
                "--steam-runtime",
                "/s",
                "--variant",
                "v",
                "--work",
                "/w",
                "--cache",
                "/c"
            ]),
            Ok(Command::Smoke {
                steam_runtime: Some(_),
                variant: Some(_),
                work: Some(_),
                cache: Some(_),
                ..
            })
        ));

        let Command::Release { installer, output } =
            parse(&["release", "--installer", "built/fenix-patch"]).unwrap()
        else {
            panic!("release");
        };
        assert_eq!(
            (installer, output),
            (PathBuf::from("built/fenix-patch"), None)
        );
    }

    #[test]
    fn options_can_be_shortened_and_repeated_as_with_argparse() {
        assert!(matches!(
            parse(&[
                "build", "--prep", "--rec", "--j", "2", "--build", "x", "--jobs", "4"
            ]),
            Ok(Command::Build {
                prepare_only: true,
                record_build: true,
                jobs: 4,
                build_dir: Some(_)
            })
        ));
        assert!(matches!(
            parse(&["smoke", "--run", "/r", "--steam", "/s", "--var", "v"]),
            Ok(Command::Smoke {
                steam_runtime: Some(_),
                variant: Some(_),
                ..
            })
        ));
        assert!(matches!(
            parse(&["release", "--inst", "a", "--installer", "b", "--out", "d"]),
            Ok(Command::Release {
                output: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn what_a_command_needs_must_be_given() {
        for arguments in [
            &["build-proton"][..],
            &["build-proton", "cachyos-11"],
            &["build-proton", "--build-dir", "/b"],
            &["smoke"],
            &["smoke", "--variant", "cachyos-11"],
            &["release"],
            &["release", "--output", "dist"],
        ] {
            assert_eq!(
                parse(arguments).unwrap_err(),
                ErrorKind::MissingRequiredArgument,
                "{arguments:?}"
            );
        }
        assert_eq!(
            parse(&["build", "--jobs", "four"]).unwrap_err(),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse(&["build", "--runner", "/r"]).unwrap_err(),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            parse(&["build", "extra"]).unwrap_err(),
            ErrorKind::UnknownArgument
        );
        assert_eq!(parse(&["test"]).unwrap_err(), ErrorKind::InvalidSubcommand);
        assert_eq!(
            parse(&[]).unwrap_err(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert_eq!(parse(&["--help"]).unwrap_err(), ErrorKind::DisplayHelp);
    }
}
