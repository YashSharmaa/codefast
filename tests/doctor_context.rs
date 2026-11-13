//! `codefast doctor context`: what a harness's configuration puts into every request, set
//! against what the project contains. It reads the harness's files and changes none.

#[path = "setup_support/mod.rs"]
mod sandbox;

use sandbox::*;
use serde_json::json;

fn skill(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\nThe body is not loaded until the skill is used.\n")
}

/// The project lives below the home directory, as projects do: what lies above the home
/// directory of the machine the tests run on is then none of the report's business.
fn project(sb: &Sandbox) -> std::path::PathBuf {
    sb.home("code/demo")
}

fn ask(sb: &Sandbox, envs: &[(&str, &str)]) -> String {
    sb.run_in(&project(sb), &["doctor", "context"], envs).ok().to_string()
}

/// A Rust project, and a Claude Code configuration with two plugins, three skills of the
/// user's own, a sub-agent and an instruction file.
fn configured(tag: &str) -> Sandbox {
    let sb = Sandbox::new(tag);
    let root = project(&sb);
    write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&root.join("Cargo.toml"), "[package]\nname = \"demo\"\n\n[dependencies]\nserde = \"1\"\ntokio = \"1\"\n");
    write(&root.join("src/main.rs"), "fn main() {}\n");
    write(&root.join("AGENTS.md"), "# demo\n\nBuild with cargo.\n");
    let video = sb.home(".claude/plugins/cache/shop/video-kit/1.0.0");
    let rusty = sb.home(".claude/plugins/cache/shop/rust-kit/2.1.0");
    for (name, text) in [
        ("captions", "Add animated captions to a talking-head video and export the clip for social media."),
        ("render", "Render a composition to MP4 with the timeline, transitions and audio ducking applied."),
        ("storyboard", "Plan the scenes of a product launch film before any footage is animated or rendered."),
    ] {
        write(&video.join(format!("skills/{name}/SKILL.md")), &skill(name, text));
    }
    write(&video.join(".claude-plugin/plugin.json"), &json!({"name": "video-kit", "mcpServers": {"media": {"command": "media-mcp"}}}).to_string());
    let long = "Explain a borrow checker error in Rust and propose the smallest change that satisfies it. ".repeat(6);
    write(&rusty.join("skills/lifetimes/SKILL.md"), &skill("lifetimes", long.trim()));
    write(&rusty.join("agents/crate-auditor.md"), "---\nname: crate-auditor\ndescription: Audits the dependencies of a Cargo workspace for unmaintained crates.\n---\nYou audit crates.\n");
    write(
        &sb.home(".claude/plugins/installed_plugins.json"),
        &json!({"version": 2, "plugins": {
            "video-kit@shop": [{"scope": "user", "installPath": video, "version": "1.0.0"}],
            "rust-kit@shop": [{"scope": "user", "installPath": rusty, "version": "2.1.0"}],
        }})
        .to_string(),
    );
    write(&sb.home(".claude/settings.json"), &json!({"enabledPlugins": {"video-kit@shop": true, "rust-kit@shop": true}}).to_string());
    write(&sb.home(".claude/skills/slides/SKILL.md"), &skill("slides", "Build a slide deck for a live presentation with speaker notes and fragment reveals."));
    let long = "Review the changed code of a pull request for bugs before it is merged. ".repeat(8);
    write(&sb.home(".claude/skills/reviewer/SKILL.md"), &skill("reviewer", long.trim()));
    write(&sb.home(".claude/skills/hidden/SKILL.md"), "---\nname: hidden\ndescription: Never shown to the model.\ndisable-model-invocation: true\n---\n");
    write(&sb.home(".claude/agents/translator.md"), "---\nname: translator\ndescription: Translates marketing copy into French, German and Japanese.\n---\nYou translate.\n");
    write(&sb.home(".claude/CLAUDE.md"), "# House rules\n\nAlways answer in British English. Never push to main.\n");
    sb
}

#[test]
fn the_report_costs_every_part_and_names_what_the_project_has_no_use_for() {
    let sb = configured("context");
    let text = ask(&sb, &[]);
    assert!(text.starts_with("Claude Code: about "), "{text}");
    assert!(text.contains(" 2 plugins, listing 5 skills, commands and agents\n"), "{text}");
    assert!(text.contains(" 2 skills of your own\n") && text.contains(" 1 sub-agents\n"), "{text}");
    assert!(text.contains(" 2 instruction files, loaded in full\n"), "{text}");
    assert!(text.contains(" 1 MCP servers declared"), "{text}");
    assert!(text.contains("This project: rust, and "), "{text}");
    // The video plugin, the slides skill and the translator have nothing to do with a Rust
    // project. The Rust plugin and the reviewer do, each for its own reason.
    let (unmatched, rest) = text.split_once("The rest, heaviest first:").expect("both lists");
    assert!(unmatched.contains("Nothing in this project's files matches 3 parts"), "{text}");
    for part in ["plugin video-kit@shop  (captions, render, storyboard)", "skill slides  (", "agent translator  ("] {
        assert!(unmatched.contains(part), "{part} in:\n{text}");
    }
    assert!(!unmatched.contains("rust-kit") && !unmatched.contains("reviewer") && !text.contains("hidden"), "{text}");
    assert!(rest.contains("plugin rust-kit@shop  lifetimes, crate-auditor  (this project has: cargo)"), "{text}");
    assert!(rest.lines().any(|l| l.contains("skill reviewer  ") && l.ends_with("(about software work in general)")), "{text}");
    // How to switch each off for this project, and nothing is switched off by asking.
    assert!(text.contains("  claude plugin disable video-kit@shop --scope local\n"), "{text}");
    assert!(text.contains("  in .claude/settings.local.json: \"skillOverrides\": {\"slides\": \"off\"}\n"), "{text}");
    assert!(text.contains("  in .claude/settings.local.json: \"permissions\": {\"deny\": [\"Agent(translator)\"]}\n"), "{text}");
    assert!(!project(&sb).join(".claude").exists(), "the report writes nothing");
}

#[test]
fn what_is_already_off_for_the_project_is_not_counted() {
    let sb = configured("context-off");
    write(&project(&sb).join(".claude/settings.local.json"), &json!({"enabledPlugins": {"video-kit@shop": false}, "skillOverrides": {"slides": "off"}}).to_string());
    let text = ask(&sb, &[]);
    assert!(text.contains(" 1 plugins, listing 2 skills, commands and agents\n") && text.contains(" 1 skills of your own\n"), "{text}");
    assert!(!text.contains("video-kit") && !text.contains("slides"), "{text}");
}

#[test]
fn an_instruction_file_loaded_twice_is_pointed_out() {
    // A second account keeps its configuration elsewhere, and the first account's CLAUDE.md
    // lies in a directory above the project: the harness reads it as a parent's instructions.
    let sb = configured("context-twice");
    let rules = read(&sb.home(".claude/CLAUDE.md"));
    write(&sb.home(".claude-work/CLAUDE.md"), &rules);
    let work = sb.home(".claude-work");
    let text = ask(&sb, &[("CLAUDE_CONFIG_DIR", work.to_str().unwrap())]);
    assert!(text.contains("Loaded twice, word for word: ~/.claude/CLAUDE.md (parent directory) repeats ~/.claude-work/CLAUDE.md (user)"), "{text}");
    assert!(text.contains(&format!("\"claudeMdExcludes\": [\"{}\"]", sb.home(".claude/CLAUDE.md").display())), "{text}");
}

#[test]
fn without_a_configuration_there_is_nothing_to_report() {
    let sb = Sandbox::new("context-none");
    write(&project(&sb).join(".git/HEAD"), "ref: refs/heads/main\n");
    assert_eq!(ask(&sb, &[]), "Claude Code: nothing of its configuration was found on this machine.\n");
}
