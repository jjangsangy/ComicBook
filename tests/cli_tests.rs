use clap::Parser;
use clap_complete::Shell;
use comic_book::clamp::Approach;
use comic_book::cli::{generate_completions, Cli, Commands};
use std::path::PathBuf;

#[test]
fn test_cli_convert_parsing() {
    let args = vec!["comic-book", "convert", "dir1", "dir2", "--to", "cbz"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Convert { directories, to } => {
            assert_eq!(
                directories,
                vec![PathBuf::from("dir1"), PathBuf::from("dir2")]
            );
            assert_eq!(to, "cbz");
        }
        _ => panic!("Expected Convert command"),
    }
}

#[test]
fn test_cli_convert_file_parsing() {
    let args = vec!["comic-book", "convert", "issue1.cbz", "--to", "cbr"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Convert { directories, to } => {
            assert_eq!(directories, vec![PathBuf::from("issue1.cbz")]);
            assert_eq!(to, "cbr");
        }
        _ => panic!("Expected Convert command"),
    }
}

#[test]
fn test_cli_convert_to_dir_parsing() {
    let args = vec!["comic-book", "convert", "issue1.cbz", "--to", "dir"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Convert { directories, to } => {
            assert_eq!(directories, vec![PathBuf::from("issue1.cbz")]);
            assert_eq!(to, "dir");
        }
        _ => panic!("Expected Convert command"),
    }
}

#[test]
fn test_cli_clamp_parsing_defaults() {
    let args = vec!["comic-book", "clamp", "my_comics"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Clamp {
            input_dir,
            output_dir,
            size_threshold,
            approach,
            workers,
        } => {
            assert_eq!(input_dir, PathBuf::from("my_comics"));
            assert_eq!(output_dir, PathBuf::from("Results"));
            assert_eq!(size_threshold, 5_000_000);
            assert_eq!(approach, Approach::Split);
            assert!(workers >= 1);
        }
        _ => panic!("Expected Clamp command"),
    }
}

#[test]
fn test_cli_clamp_parsing_custom_options() {
    let args = vec![
        "comic-book",
        "clamp",
        "chapter.cbz",
        "-o",
        "CustomOut",
        "-s",
        "800",
        "-a",
        "max-width",
        "-w",
        "4",
    ];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Clamp {
            input_dir,
            output_dir,
            size_threshold,
            approach,
            workers,
        } => {
            assert_eq!(input_dir, PathBuf::from("chapter.cbz"));
            assert_eq!(output_dir, PathBuf::from("CustomOut"));
            assert_eq!(size_threshold, 800);
            assert_eq!(approach, Approach::MaxWidth);
            assert_eq!(workers, 4);
        }
        _ => panic!("Expected Clamp command"),
    }
}

#[test]
fn test_cli_invalid_arguments() {
    // Missing required `--to`
    assert!(Cli::try_parse_from(vec!["comic-book", "convert", "dir1"]).is_err());
    assert!(Cli::try_parse_from(vec!["comic-book", "convert", "issue.cbz"]).is_err());

    // Invalid --to format
    assert!(Cli::try_parse_from(vec!["comic-book", "convert", "dir1", "--to", "invalid"]).is_err());
    assert!(
        Cli::try_parse_from(vec!["comic-book", "convert", "dir1", "--to", "directory"]).is_err()
    );
    assert!(Cli::try_parse_from(vec!["comic-book", "convert", "dir1", "--to", "folder"]).is_err());

    // Invalid approach
    assert!(Cli::try_parse_from(vec![
        "comic-book",
        "clamp",
        "dir1",
        "--approach",
        "non-existent"
    ])
    .is_err());

    // Unknown command
    assert!(Cli::try_parse_from(vec!["comic-book", "unknown-cmd"]).is_err());
}

#[test]
fn test_cli_completions_parsing() {
    // Positional argument
    let args = vec!["comic-book", "completions", "bash"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Completions { shell, shell_flag } => {
            assert_eq!(shell, Some(Shell::Bash));
            assert_eq!(shell_flag, None);
        }
        _ => panic!("Expected Completions command"),
    }

    // Subcommand alias "completion"
    let args = vec!["comic-book", "completion", "zsh"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Completions { shell, shell_flag } => {
            assert_eq!(shell, Some(Shell::Zsh));
            assert_eq!(shell_flag, None);
        }
        _ => panic!("Expected Completions command"),
    }

    // Flag option `-s` / `--shell`
    let args = vec!["comic-book", "completions", "--shell", "fish"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Completions { shell, shell_flag } => {
            assert_eq!(shell, None);
            assert_eq!(shell_flag, Some(Shell::Fish));
        }
        _ => panic!("Expected Completions command"),
    }

    // Without arguments (for auto-detection)
    let args = vec!["comic-book", "completions"];
    let cli = Cli::try_parse_from(args).unwrap();
    match cli.command {
        Commands::Completions { shell, shell_flag } => {
            assert_eq!(shell, None);
            assert_eq!(shell_flag, None);
        }
        _ => panic!("Expected Completions command"),
    }

    // Invalid shell
    assert!(Cli::try_parse_from(vec!["comic-book", "completions", "invalid_shell"]).is_err());
}

#[test]
fn test_generate_completions() {
    for shell in [
        Shell::Bash,
        Shell::Elvish,
        Shell::Fish,
        Shell::PowerShell,
        Shell::Zsh,
    ] {
        let mut buf = Vec::new();
        generate_completions(shell, &mut buf);
        let output = String::from_utf8(buf).expect("Completions output should be valid UTF-8");
        assert!(!output.is_empty());
        assert!(
            output.contains("comic-book"),
            "Completions for {:?} should mention binary name 'comic-book'",
            shell
        );
    }
}
