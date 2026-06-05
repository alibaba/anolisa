use super::validators;

pub struct ReadonlySpec {
    pub command: &'static str,
    pub validator: Validator,
}

pub enum Validator {
    Bare,
    Generic(GenericSpec),
    Subcommand(SubcommandSpec),
    VersionCheck(&'static [&'static str]),
    Custom(fn(&[String]) -> bool),
}

pub struct GenericSpec {
    pub short_flags: &'static str,
    pub long_flags: &'static [&'static str],
    pub value_flags: &'static [(&'static str, Option<u32>)],
    pub deny_flags: &'static [&'static str],
    pub path_mode: PathMode,
    pub bare_number_max: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    None,
    Optional,
    Required,
    Unchecked,
}

pub struct SubcommandSpec {
    pub deny_args: &'static [&'static str],
    pub subcommands: &'static [(&'static str, Validator)],
}

// ── Registry ──

pub static READONLY_SPECS: &[ReadonlySpec] = &[
    // ── Bare commands (no args) ──
    ReadonlySpec {
        command: "pwd",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "whoami",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "hostname",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "date",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "uptime",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "vm_stat",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "nproc",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "sw_vers",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "arch",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "tty",
        validator: Validator::Bare,
    },
    ReadonlySpec {
        command: "groups",
        validator: Validator::Bare,
    },
    // ── Flag-only commands (no positional args) ──
    ReadonlySpec {
        command: "uname",
        validator: Validator::Generic(GenericSpec {
            short_flags: "amnprsv",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::None,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "id",
        validator: Validator::Generic(GenericSpec {
            short_flags: "ugGnr",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::None,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "free",
        validator: Validator::Generic(GenericSpec {
            short_flags: "bmghtlsw",
            long_flags: &["--human", "--bytes", "--kilo", "--mega", "--giga", "--tera"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::None,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "xcode-select",
        validator: Validator::Generic(GenericSpec {
            short_flags: "p",
            long_flags: &["--print-path"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::None,
            bare_number_max: 0,
        }),
    },
    // ── Flags + required path args ──
    ReadonlySpec {
        command: "ls",
        validator: Validator::Generic(GenericSpec {
            short_flags: "1AFGHLRSadhlrt",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "cat",
        validator: Validator::Generic(GenericSpec {
            short_flags: "nbs",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "wc",
        validator: Validator::Generic(GenericSpec {
            short_flags: "lwcmL",
            long_flags: &[
                "--lines",
                "--words",
                "--chars",
                "--bytes",
                "--max-line-length",
            ],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "file",
        validator: Validator::Generic(GenericSpec {
            short_flags: "biLNpz",
            long_flags: &["--mime", "--mime-type", "--brief"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "stat",
        validator: Validator::Generic(GenericSpec {
            short_flags: "fLt",
            long_flags: &["--format", "--printf", "--terse"],
            value_flags: &[("-c", None)],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "diff",
        validator: Validator::Generic(GenericSpec {
            short_flags: "qrsuyNaibBwEZ",
            long_flags: &[
                "--brief",
                "--unified",
                "--color",
                "--color=auto",
                "--color=always",
                "--color=never",
                "--stat",
                "--no-dereference",
            ],
            value_flags: &[("-U", Some(10_000))],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "comm",
        validator: Validator::Generic(GenericSpec {
            short_flags: "123i",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "md5sum",
        validator: Validator::Generic(GenericSpec {
            short_flags: "bct",
            long_flags: &["--check", "--tag"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "shasum",
        validator: Validator::Generic(GenericSpec {
            short_flags: "bcta",
            long_flags: &["--check", "--tag"],
            value_flags: &[("-a", None)],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "sha256sum",
        validator: Validator::Generic(GenericSpec {
            short_flags: "bct",
            long_flags: &["--check", "--tag"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "realpath",
        validator: Validator::Generic(GenericSpec {
            short_flags: "eLmPqsz",
            long_flags: &[
                "--canonicalize-existing",
                "--canonicalize-missing",
                "--no-symlinks",
            ],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "readlink",
        validator: Validator::Generic(GenericSpec {
            short_flags: "fenqsz",
            long_flags: &["--canonicalize", "--canonicalize-existing", "--no-newline"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Required,
            bare_number_max: 0,
        }),
    },
    // ── Flags + optional path args ──
    ReadonlySpec {
        command: "du",
        validator: Validator::Generic(GenericSpec {
            short_flags: "shkcmgabxlLHP",
            long_flags: &[
                "--human-readable",
                "--summarize",
                "--total",
                "--apparent-size",
                "--bytes",
                "--one-file-system",
            ],
            value_flags: &[("-d", Some(20)), ("--max-depth", Some(20))],
            deny_flags: &[],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "df",
        validator: Validator::Generic(GenericSpec {
            short_flags: "hHkmgPTil",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &["--output"],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "sort",
        validator: Validator::Generic(GenericSpec {
            short_flags: "bdfiMnrRsugkt",
            long_flags: &[
                "--numeric-sort",
                "--reverse",
                "--unique",
                "--stable",
                "--human-numeric-sort",
                "--check",
            ],
            value_flags: &[],
            deny_flags: &["-o", "--output"],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "uniq",
        validator: Validator::Generic(GenericSpec {
            short_flags: "cduifszw",
            long_flags: &["--count", "--repeated", "--unique", "--ignore-case"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "cut",
        validator: Validator::Generic(GenericSpec {
            short_flags: "s",
            long_flags: &["--complement", "--only-delimited"],
            value_flags: &[
                ("-d", None),
                ("-f", None),
                ("-c", None),
                ("-b", None),
                ("--delimiter", None),
                ("--fields", None),
                ("--characters", None),
                ("--bytes", None),
            ],
            deny_flags: &["--output-delimiter"],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "fold",
        validator: Validator::Generic(GenericSpec {
            short_flags: "bs",
            long_flags: &["--bytes", "--spaces"],
            value_flags: &[("-w", Some(10_000)), ("--width", Some(10_000))],
            deny_flags: &[],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "expand",
        validator: Validator::Generic(GenericSpec {
            short_flags: "i",
            long_flags: &["--initial"],
            value_flags: &[("-t", None), ("--tabs", None)],
            deny_flags: &[],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "unexpand",
        validator: Validator::Generic(GenericSpec {
            short_flags: "a",
            long_flags: &["--all", "--first-only"],
            value_flags: &[("-t", None), ("--tabs", None)],
            deny_flags: &[],
            path_mode: PathMode::Optional,
            bare_number_max: 0,
        }),
    },
    // ── Unchecked positional args ──
    ReadonlySpec {
        command: "which",
        validator: Validator::Generic(GenericSpec {
            short_flags: "a",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "dirname",
        validator: Validator::Generic(GenericSpec {
            short_flags: "z",
            long_flags: &["--zero"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "basename",
        validator: Validator::Generic(GenericSpec {
            short_flags: "az",
            long_flags: &["--multiple", "--zero"],
            value_flags: &[("-s", None), ("--suffix", None)],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "printenv",
        validator: Validator::Generic(GenericSpec {
            short_flags: "0",
            long_flags: &["--null"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "echo",
        validator: Validator::Generic(GenericSpec {
            short_flags: "neE",
            long_flags: &[],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
    ReadonlySpec {
        command: "tr",
        validator: Validator::Generic(GenericSpec {
            short_flags: "cCds",
            long_flags: &["--complement", "--delete", "--squeeze-repeats"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
    // ── Process inspection ──
    ReadonlySpec {
        command: "pgrep",
        validator: Validator::Generic(GenericSpec {
            short_flags: "cdfgilnotuUvxP",
            long_flags: &[
                "--count",
                "--full",
                "--list-name",
                "--list-full",
                "--newest",
                "--oldest",
                "--inverse",
            ],
            value_flags: &[
                ("-G", None),
                ("-P", None),
                ("-s", None),
                ("-u", None),
                ("-U", None),
            ],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
    // ── Version checks ──
    ReadonlySpec {
        command: "rustc",
        validator: Validator::VersionCheck(&["--version", "-V"]),
    },
    ReadonlySpec {
        command: "rustup",
        validator: Validator::VersionCheck(&["--version"]),
    },
    ReadonlySpec {
        command: "node",
        validator: Validator::VersionCheck(&["--version", "-v"]),
    },
    ReadonlySpec {
        command: "npm",
        validator: Validator::VersionCheck(&["--version", "-v"]),
    },
    ReadonlySpec {
        command: "python",
        validator: Validator::VersionCheck(&["--version", "-V"]),
    },
    ReadonlySpec {
        command: "python3",
        validator: Validator::VersionCheck(&["--version", "-V"]),
    },
    ReadonlySpec {
        command: "pip",
        validator: Validator::VersionCheck(&["--version", "-V"]),
    },
    ReadonlySpec {
        command: "pip3",
        validator: Validator::VersionCheck(&["--version", "-V"]),
    },
    ReadonlySpec {
        command: "java",
        validator: Validator::VersionCheck(&["-version", "--version"]),
    },
    ReadonlySpec {
        command: "javac",
        validator: Validator::VersionCheck(&["-version", "--version"]),
    },
    ReadonlySpec {
        command: "ruby",
        validator: Validator::VersionCheck(&["--version", "-v"]),
    },
    ReadonlySpec {
        command: "swift",
        validator: Validator::VersionCheck(&["--version"]),
    },
    ReadonlySpec {
        command: "clang",
        validator: Validator::VersionCheck(&["--version"]),
    },
    ReadonlySpec {
        command: "gcc",
        validator: Validator::VersionCheck(&["--version", "-v"]),
    },
    ReadonlySpec {
        command: "g++",
        validator: Validator::VersionCheck(&["--version", "-v"]),
    },
    ReadonlySpec {
        command: "cmake",
        validator: Validator::VersionCheck(&["--version"]),
    },
    ReadonlySpec {
        command: "make",
        validator: Validator::VersionCheck(&["--version", "-v"]),
    },
    ReadonlySpec {
        command: "dotnet",
        validator: Validator::VersionCheck(&["--version"]),
    },
    // ── Custom validators (structural complexity) ──
    ReadonlySpec {
        command: "head",
        validator: Validator::Custom(validators::is_readonly_head),
    },
    ReadonlySpec {
        command: "tail",
        validator: Validator::Custom(validators::is_readonly_tail),
    },
    ReadonlySpec {
        command: "grep",
        validator: Validator::Custom(validators::is_readonly_grep),
    },
    ReadonlySpec {
        command: "rg",
        validator: Validator::Custom(validators::is_readonly_rg),
    },
    ReadonlySpec {
        command: "find",
        validator: Validator::Custom(validators::is_readonly_find),
    },
    ReadonlySpec {
        command: "ps",
        validator: Validator::Custom(validators::is_readonly_ps),
    },
    ReadonlySpec {
        command: "sysctl",
        validator: Validator::Custom(validators::is_readonly_sysctl),
    },
    ReadonlySpec {
        command: "top",
        validator: Validator::Custom(validators::is_bounded_top_snapshot),
    },
    ReadonlySpec {
        command: "env",
        validator: Validator::Custom(validators::is_readonly_env),
    },
    // ── Subcommand dispatch: git ──
    ReadonlySpec {
        command: "git",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &["-c", "--ext-diff", "--textconv", "--output", "--exec-path"],
            subcommands: &[
                (
                    "status",
                    Validator::Generic(GenericSpec {
                        short_flags: "s",
                        long_flags: &[
                            "--short",
                            "--branch",
                            "--porcelain",
                            "--porcelain=v1",
                            "--porcelain=v2",
                            "--ignored",
                            "--untracked-files",
                            "--untracked-files=no",
                            "--untracked-files=normal",
                            "--untracked-files=all",
                        ],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::None,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "diff",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[
                            "--stat",
                            "--shortstat",
                            "--numstat",
                            "--summary",
                            "--name-only",
                            "--name-status",
                            "--cached",
                            "--staged",
                            "--check",
                            "--no-ext-diff",
                        ],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "log",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[
                            "--oneline",
                            "--decorate",
                            "--stat",
                            "--shortstat",
                            "--name-only",
                            "--name-status",
                            "--no-ext-diff",
                            "--graph",
                            "--all",
                            "--follow",
                            "--first-parent",
                            "--reverse",
                            "--no-merges",
                            "--merges",
                        ],
                        value_flags: &[
                            ("-n", Some(10_000)),
                            ("--max-count", Some(10_000)),
                            ("--format", None),
                            ("--pretty", None),
                            ("--since", None),
                            ("--until", None),
                            ("--after", None),
                            ("--before", None),
                            ("--author", None),
                            ("--grep", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 10_000,
                    }),
                ),
                (
                    "show",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[
                            "--stat",
                            "--shortstat",
                            "--name-only",
                            "--name-status",
                            "--no-ext-diff",
                        ],
                        value_flags: &[("--format", None), ("--pretty", None)],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "branch",
                    Validator::Custom(validators::is_readonly_git_branch),
                ),
                (
                    "remote",
                    Validator::Generic(GenericSpec {
                        short_flags: "v",
                        long_flags: &["--verbose"],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "rev-parse",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[
                            "--short",
                            "--verify",
                            "--abbrev-ref",
                            "--show-toplevel",
                            "--git-dir",
                            "--is-inside-work-tree",
                            "--is-bare-repository",
                            "--show-cdup",
                            "--show-prefix",
                            "--absolute-git-dir",
                        ],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "blame",
                    Validator::Generic(GenericSpec {
                        short_flags: "eLftnsMpw",
                        long_flags: &[
                            "--line-porcelain",
                            "--porcelain",
                            "--root",
                            "--show-stats",
                            "--no-progress",
                        ],
                        value_flags: &[("-L", None)],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "describe",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[
                            "--tags",
                            "--always",
                            "--long",
                            "--dirty",
                            "--first-parent",
                            "--all",
                        ],
                        value_flags: &[
                            ("--abbrev", Some(40)),
                            ("--match", None),
                            ("--exclude", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "ls-files",
                    Validator::Generic(GenericSpec {
                        short_flags: "codsimktz",
                        long_flags: &[
                            "--error-unmatch",
                            "--full-name",
                            "--abbrev",
                            "--debug",
                            "--deduplicate",
                        ],
                        value_flags: &[],
                        deny_flags: &["--delete", "--modify"],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "ls-tree",
                    Validator::Generic(GenericSpec {
                        short_flags: "rdtlz",
                        long_flags: &[
                            "--name-only",
                            "--name-status",
                            "--full-name",
                            "--full-tree",
                            "--long",
                            "--abbrev",
                        ],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "grep",
                    Validator::Generic(GenericSpec {
                        short_flags: "niclwEFP",
                        long_flags: &[
                            "--cached",
                            "--no-index",
                            "--untracked",
                            "--fixed-strings",
                            "--perl-regexp",
                            "--extended-regexp",
                            "--word-regexp",
                            "--count",
                            "--name-only",
                            "--files-with-matches",
                            "--files-without-match",
                            "--heading",
                            "--break",
                            "--show-function",
                        ],
                        value_flags: &[
                            ("-A", Some(10_000)),
                            ("-B", Some(10_000)),
                            ("-C", Some(10_000)),
                            ("-e", None),
                            ("--max-depth", Some(20)),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "shortlog",
                    Validator::Generic(GenericSpec {
                        short_flags: "sne",
                        long_flags: &["--summary", "--numbered", "--email", "--no-merges"],
                        value_flags: &[("-w", None), ("--group", None), ("--format", None)],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "rev-list",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[
                            "--count",
                            "--all",
                            "--oneline",
                            "--first-parent",
                            "--no-merges",
                            "--merges",
                            "--reverse",
                            "--ancestry-path",
                            "--topo-order",
                            "--date-order",
                        ],
                        value_flags: &[
                            ("-n", Some(10_000)),
                            ("--max-count", Some(10_000)),
                            ("--since", None),
                            ("--until", None),
                            ("--after", None),
                            ("--before", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 10_000,
                    }),
                ),
                (
                    "cat-file",
                    Validator::Generic(GenericSpec {
                        short_flags: "tpse",
                        long_flags: &["--textconv", "--batch", "--batch-check"],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "count-objects",
                    Validator::Generic(GenericSpec {
                        short_flags: "vH",
                        long_flags: &["--verbose", "--human-readable"],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::None,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "for-each-ref",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[],
                        value_flags: &[
                            ("--sort", None),
                            ("--format", None),
                            ("--count", Some(10_000)),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "name-rev",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &[
                            "--tags",
                            "--name-only",
                            "--no-undefined",
                            "--always",
                            "--stdin",
                        ],
                        value_flags: &[("--refs", None), ("--exclude", None)],
                        deny_flags: &[],
                        path_mode: PathMode::Optional,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "stash",
                    Validator::Custom(validators::is_readonly_git_stash),
                ),
                (
                    "config",
                    Validator::Custom(validators::is_readonly_git_config),
                ),
                ("tag", Validator::Custom(validators::is_readonly_git_tag)),
                (
                    "reflog",
                    Validator::Custom(validators::is_readonly_git_reflog),
                ),
            ],
        }),
    },
    // ── Subcommand dispatch: cargo ──
    ReadonlySpec {
        command: "cargo",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &[],
            subcommands: &[
                ("--version", Validator::Bare),
                (
                    "tree",
                    Validator::Generic(GenericSpec {
                        short_flags: "eidp",
                        long_flags: &[
                            "--workspace",
                            "--all-features",
                            "--no-default-features",
                            "--no-dedupe",
                            "--duplicates",
                            "--invert",
                        ],
                        value_flags: &[
                            ("--depth", Some(100)),
                            ("--target", None),
                            ("--features", None),
                            ("--format", None),
                            ("-p", None),
                            ("--package", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::None,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "metadata",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &["--no-deps", "--all-features", "--no-default-features"],
                        value_flags: &[
                            ("--format-version", None),
                            ("--features", None),
                            ("--filter-platform", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::None,
                        bare_number_max: 0,
                    }),
                ),
            ],
        }),
    },
    // ── Subcommand dispatch: docker ──
    ReadonlySpec {
        command: "docker",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &[],
            subcommands: &[
                (
                    "ps",
                    Validator::Generic(GenericSpec {
                        short_flags: "aqns",
                        long_flags: &["--no-trunc", "--latest", "--last", "--size"],
                        value_flags: &[
                            ("--format", None),
                            ("-f", None),
                            ("--filter", None),
                            ("-n", Some(10_000)),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::None,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "images",
                    Validator::Generic(GenericSpec {
                        short_flags: "aq",
                        long_flags: &["--no-trunc", "--digests"],
                        value_flags: &[("--format", None), ("-f", None), ("--filter", None)],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "inspect",
                    Validator::Generic(GenericSpec {
                        short_flags: "s",
                        long_flags: &["--size"],
                        value_flags: &[("--format", None), ("--type", None), ("-f", None)],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                ("version", Validator::Bare),
                ("info", Validator::Bare),
            ],
        }),
    },
    // ── Subcommand dispatch: kubectl ──
    ReadonlySpec {
        command: "kubectl",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &["--dry-run"],
            subcommands: &[
                (
                    "get",
                    Validator::Generic(GenericSpec {
                        short_flags: "Aw",
                        long_flags: &[
                            "--all-namespaces",
                            "--show-labels",
                            "--no-headers",
                            "--watch-only",
                            "--show-kind",
                        ],
                        value_flags: &[
                            ("-o", None),
                            ("--output", None),
                            ("-n", None),
                            ("--namespace", None),
                            ("-l", None),
                            ("--selector", None),
                            ("--field-selector", None),
                            ("--sort-by", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "describe",
                    Validator::Generic(GenericSpec {
                        short_flags: "A",
                        long_flags: &["--all-namespaces", "--show-events"],
                        value_flags: &[
                            ("-n", None),
                            ("--namespace", None),
                            ("-l", None),
                            ("--selector", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "version",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &["--client", "--short"],
                        value_flags: &[("-o", None), ("--output", None)],
                        deny_flags: &[],
                        path_mode: PathMode::None,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "top",
                    Validator::Generic(GenericSpec {
                        short_flags: "A",
                        long_flags: &["--all-namespaces", "--containers", "--no-headers"],
                        value_flags: &[
                            ("-n", None),
                            ("--namespace", None),
                            ("-l", None),
                            ("--selector", None),
                            ("--sort-by", None),
                        ],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
            ],
        }),
    },
    // ── Subcommand dispatch: go ──
    ReadonlySpec {
        command: "go",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &[],
            subcommands: &[
                ("version", Validator::Bare),
                (
                    "env",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &["-json"],
                        value_flags: &[],
                        deny_flags: &["-w", "-u"],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
            ],
        }),
    },
    // ── Subcommand dispatch: macOS ──
    ReadonlySpec {
        command: "diskutil",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &[],
            subcommands: &[
                (
                    "list",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &["-plist"],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
                (
                    "info",
                    Validator::Generic(GenericSpec {
                        short_flags: "",
                        long_flags: &["-plist", "-all"],
                        value_flags: &[],
                        deny_flags: &[],
                        path_mode: PathMode::Unchecked,
                        bare_number_max: 0,
                    }),
                ),
            ],
        }),
    },
    ReadonlySpec {
        command: "defaults",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &[],
            subcommands: &[(
                "read",
                Validator::Generic(GenericSpec {
                    short_flags: "g",
                    long_flags: &["-globalDomain"],
                    value_flags: &[],
                    deny_flags: &[],
                    path_mode: PathMode::Unchecked,
                    bare_number_max: 0,
                }),
            )],
        }),
    },
    ReadonlySpec {
        command: "xcrun",
        validator: Validator::Subcommand(SubcommandSpec {
            deny_args: &[],
            subcommands: &[(
                "--find",
                Validator::Generic(GenericSpec {
                    short_flags: "",
                    long_flags: &[],
                    value_flags: &[],
                    deny_flags: &[],
                    path_mode: PathMode::Unchecked,
                    bare_number_max: 0,
                }),
            )],
        }),
    },
    ReadonlySpec {
        command: "system_profiler",
        validator: Validator::Generic(GenericSpec {
            short_flags: "",
            long_flags: &["-xml", "-json", "-detailLevel"],
            value_flags: &[],
            deny_flags: &[],
            path_mode: PathMode::Unchecked,
            bare_number_max: 0,
        }),
    },
];

// ── Custom validators ──
