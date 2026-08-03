//! Public CLI: 24 compatibility subcommands plus the mcp/hook/host-setup
//! entry points. Output channels, exit codes and JSON field types are frozen;
//! only clap's help layout may differ from argparse (the declared P0 waiver).

use std::io::Read;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::hooks;
use crate::json::Json;
use crate::paths::{project_context_id, Layout};
use crate::{backup, cache, lifecycle, search, txn, Error, MAX_PUBLIC_ID};

#[derive(Parser)]
#[command(name = "engramark", version, about = "Engramark CLI")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Save a published memory from .mem text ("-" reads stdin).
    Save {
        text: String,
        #[arg(long)]
        lock: bool,
    },
    /// Propose a candidate memory.
    Propose {
        text: String,
        #[arg(long, default_value = "")]
        source: String,
    },
    Publish {
        id: i64,
    },
    Reject {
        id: i64,
    },
    Search {
        query: Option<String>,
        #[arg(long, default_value = "published", value_parser = ["published", "candidate", "all"])]
        scope: String,
        #[arg(long, default_value_t = 8)]
        limit: i64,
        #[arg(long, default_value = "")]
        project: String,
        #[arg(long)]
        explain: bool,
    },
    Get {
        ids: Vec<i64>,
    },
    Feedback {
        id: i64,
        #[arg(value_parser = ["+", "-"])]
        signal: String,
        #[arg(long, default_value = "")]
        note: String,
    },
    Update {
        id: i64,
        text: String,
    },
    Archive {
        id: i64,
    },
    Delete {
        id: i64,
        #[arg(long)]
        confirm: bool,
    },
    Audit,
    Scan {
        #[arg(long, default_value = "default")]
        session: String,
        #[arg(long)]
        budget: Option<i64>,
        #[arg(long, default_value = "")]
        project: String,
        #[arg(long)]
        hook_fast: bool,
    },
    ScanCommit {
        #[arg(long)]
        hook_fast: bool,
    },
    ScanCancel {
        #[arg(long)]
        hook_fast: bool,
    },
    PrepareCache {
        #[arg(long)]
        if_needed: bool,
    },
    CandidateList {
        #[arg(long)]
        count: bool,
        #[arg(long, default_value = "")]
        project: String,
    },
    Rebuild,
    Recover,
    MigrateV1,
    Diagnose {
        #[arg(long)]
        full: bool,
    },
    Backup {
        destination: String,
    },
    Rollback {
        source: String,
        #[arg(long)]
        confirm: bool,
    },
    ProjectId {
        cwd: String,
        #[arg(long)]
        authoritative: bool,
    },
    Top {
        #[arg(long, default_value_t = 3)]
        limit: i64,
        #[arg(long, default_value = "")]
        project: String,
        #[arg(long)]
        human: bool,
    },
    /// Run the MCP stdio server.
    Mcp,
    /// Internal host hook entry points.
    Hook {
        event: String,
    },
    /// Manage Codex/OpenCode wiring.
    HostSetup {
        action: String,
        #[arg(long)]
        home: Option<String>,
        #[arg(long)]
        app_root: Option<String>,
        #[arg(long)]
        data_home: Option<String>,
        #[arg(long, default_value = "auto", value_parser = ["auto", "yes", "no"])]
        codex: String,
        #[arg(long, default_value = "auto", value_parser = ["auto", "yes", "no"])]
        opencode: String,
        #[arg(long)]
        project: Option<String>,
    },
}

fn print_json(value: &Json) {
    println!("{}", value.dumps());
}

fn public_id(value: i64) -> crate::Result<i64> {
    if value < 1 {
        return Err(Error::core("记忆编号必须大于等于 1"));
    }
    if value > MAX_PUBLIC_ID {
        return Err(Error::core(format!("记忆编号超过安全上限 {MAX_PUBLIC_ID}")));
    }
    Ok(value)
}

fn read_stdin() -> String {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    input
}

fn text_or_stdin(text: &str) -> String {
    if text == "-" {
        read_stdin()
    } else {
        text.to_string()
    }
}

fn default_project(layout: &Layout, project: &str) -> String {
    if !project.is_empty() {
        return project.to_string();
    }
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string));
    project_context_id(cwd.as_deref(), false, layout)
}

pub fn run() -> i32 {
    let cli = Cli::parse();
    let layout = Layout::discover();
    // project-id and the hook-fast paths must not resurrect a removed data
    // directory merely because stale host wiring invokes them.
    let skip_layout = matches!(
        &cli.command,
        Command::ProjectId { .. }
            | Command::Scan {
                hook_fast: true,
                ..
            }
            | Command::ScanCommit { .. }
            | Command::ScanCancel { .. }
            | Command::Hook { .. }
            | Command::HostSetup { .. }
            | Command::Mcp
    );
    if !skip_layout {
        if let Err(err) = layout.ensure() {
            return report_error(&Error::core(err.to_string()));
        }
    }
    match dispatch(&layout, &cli.command) {
        Ok(()) => 0,
        Err(err) => {
            if matches!(cli.command, Command::HostSetup { .. }) {
                // host-setup keeps its own channel: plain text on stderr, exit 2.
                eprintln!("错误：{}", strip_marker(&err.to_string()));
                return 2;
            }
            report_error(&err)
        }
    }
}

fn strip_marker(message: &str) -> &str {
    message
        .strip_prefix('\u{1}')
        .or_else(|| message.strip_prefix('\u{2}'))
        .unwrap_or(message)
}

fn report_error(err: &Error) -> i32 {
    let payload = crate::jobject! {
        "ok" => false,
        "error" => strip_marker(&err.to_string()),
    };
    eprintln!("{}", payload.dumps());
    1
}

fn dispatch(layout: &Layout, command: &Command) -> crate::Result<()> {
    match command {
        Command::Save { text, lock } => {
            let card = lifecycle::write_new_card(
                layout,
                &text_or_stdin(text),
                "published",
                "user",
                *lock,
            )?;
            print_json(&crate::jobject! {
                "ok" => true,
                "id" => card.id,
                "deduplicated" => card.deduplicated,
                "path" => crate::paths::require_unicode(&layout.card_path(card.id))
                    .map_err(|err| Error::core(err.to_string()))?,
            });
        }
        Command::Propose { text, source } => {
            let source = if source.is_empty() {
                "self:unknown"
            } else {
                source
            };
            let card = lifecycle::write_new_card(
                layout,
                &text_or_stdin(text),
                "candidate",
                source,
                false,
            )?;
            print_json(&crate::jobject! {
                "ok" => true,
                "id" => card.id,
                "deduplicated" => card.deduplicated,
            });
        }
        Command::Publish { id } => {
            let card = lifecycle::publish(layout, public_id(*id)?, None)?;
            print_json(&crate::jobject! {"ok" => true, "id" => card.id});
        }
        Command::Reject { id } => {
            lifecycle::reject(layout, public_id(*id)?, None)?;
            print_json(&crate::jobject! {"ok" => true, "id" => *id});
        }
        Command::Search {
            query,
            scope,
            limit,
            project,
            explain,
        } => {
            let project = default_project(layout, project);
            let rows = search::search(
                layout,
                query.as_deref().unwrap_or(""),
                scope,
                *limit,
                &project,
                None,
            )?;
            print_json(&crate::jobject! {
                "results" => Json::Array(rows.iter().map(|row| {
                    Json::Str(crate::textops::index_line(row, *explain))
                }).collect()),
            });
        }
        Command::Get { ids } => {
            let ids: Vec<i64> = ids
                .iter()
                .map(|id| public_id(*id))
                .collect::<crate::Result<_>>()?;
            let cards = lifecycle::get_cards(layout, &ids, None)?;
            print_json(&crate::jobject! {
                "cards" => Json::Array(cards.iter().map(|card| crate::jobject! {
                    "id" => card.id,
                    "text" => card.text.clone(),
                    "truncated" => card.truncated,
                }).collect()),
            });
        }
        Command::Feedback { id, signal, note } => {
            let _ = note;
            let result = lifecycle::feedback(layout, public_id(*id)?, signal, None)?;
            let mut pairs = vec![("ok".to_string(), Json::Bool(true))];
            if let Json::Object(fields) = result {
                pairs.extend(fields);
            }
            print_json(&Json::Object(pairs));
        }
        Command::Update { id, text } => {
            let card = lifecycle::update_card(layout, public_id(*id)?, &text_or_stdin(text))?;
            print_json(&crate::jobject! {"ok" => true, "id" => card.id});
        }
        Command::Archive { id } => {
            let card = lifecycle::archive_card(layout, public_id(*id)?, None)?;
            print_json(&crate::jobject! {"ok" => true, "id" => card.id, "status" => card.status});
        }
        Command::Delete { id, confirm } => {
            let card = lifecycle::tombstone_card(layout, public_id(*id)?, *confirm, None)?;
            print_json(&crate::jobject! {"ok" => true, "id" => card.id, "status" => card.status});
        }
        Command::Audit => {
            let report = lifecycle::audit(layout, None)?;
            println!("{}", report.dumps_indent1());
        }
        Command::Scan {
            session,
            budget,
            project,
            hook_fast,
        } => {
            if *hook_fast {
                let payload = hooks::read_hook_stdin(&[
                    "protocol_version",
                    "host",
                    "session_id",
                    "project_path",
                    "text",
                    "budget",
                ])?;
                let result = hooks::hook_fast_scan(layout, &payload)?;
                println!("{}", result.dumps_canonical());
                return Ok(());
            }
            let text = read_stdin();
            let project = default_project(layout, project);
            let result = hooks::scan_text(layout, &text, session, *budget, &project)?;
            print_json(&result);
        }
        Command::ScanCommit { hook_fast } => {
            if !hook_fast {
                return Err(Error::HookProtocol(
                    "scan-commit requires --hook-fast".into(),
                ));
            }
            let payload = hooks::read_hook_stdin(&[
                "protocol_version",
                "host",
                "session_key",
                "reservation_id",
            ])?;
            let result = hooks::hook_control(layout, &payload, true)?;
            println!("{}", result.dumps_canonical());
        }
        Command::ScanCancel { hook_fast } => {
            if !hook_fast {
                return Err(Error::HookProtocol(
                    "scan-cancel requires --hook-fast".into(),
                ));
            }
            let payload = hooks::read_hook_stdin(&[
                "protocol_version",
                "host",
                "session_key",
                "reservation_id",
            ])?;
            let result = hooks::hook_control(layout, &payload, false)?;
            println!("{}", result.dumps_canonical());
        }
        Command::PrepareCache { if_needed } => {
            if !if_needed {
                return Err(Error::core("prepare-cache requires --if-needed"));
            }
            let report = cache::prepare_cache_if_needed(layout)?;
            let mut pairs = vec![("ok".to_string(), Json::Bool(true))];
            if let Json::Object(fields) = report {
                pairs.extend(fields);
            }
            print_json(&Json::Object(pairs));
        }
        Command::CandidateList { count, project } => {
            let project_ref = if project.is_empty() {
                None
            } else {
                Some(project.as_str())
            };
            let cards = lifecycle::candidate_list(layout, project_ref)?;
            if *count {
                print_json(&crate::jobject! {"count" => cards.len() as i64});
            } else {
                print_json(&crate::jobject! {
                    "candidates" => Json::Array(cards.iter().map(|card| crate::jobject! {
                        "id" => card.id,
                        "title" => card.title.clone(),
                        "entities" => Json::Array(card.entities.iter().map(|e| Json::Str(e.clone())).collect()),
                        "source" => card.source.clone(),
                    }).collect()),
                });
            }
        }
        Command::Rebuild => {
            let report = cache::rebuild(layout)?;
            let mut pairs = vec![
                ("ok".to_string(), Json::Bool(true)),
                ("fts5".to_string(), Json::Bool(true)),
            ];
            if let Json::Object(fields) = report {
                pairs.extend(fields);
            }
            print_json(&Json::Object(pairs));
        }
        Command::Recover => {
            let recovered = txn::recover_transactions(layout)?;
            print_json(&crate::jobject! {"ok" => true, "recovered" => Json::Array(recovered)});
        }
        Command::MigrateV1 => {
            let report = backup::migrate_v1(layout)?;
            print_json(&report);
        }
        Command::Diagnose { full } => {
            let report = backup::diagnose(layout, *full)?;
            println!("{}", report.dumps_indent1());
        }
        Command::Backup { destination } => {
            let report = backup::backup_snapshot(layout, &PathBuf::from(destination))?;
            print_json(&report);
        }
        Command::Rollback { source, confirm } => {
            let report = backup::rollback_snapshot(layout, &PathBuf::from(source), *confirm)?;
            print_json(&report);
        }
        Command::ProjectId { cwd, authoritative } => {
            let project = project_context_id(Some(cwd), *authoritative, layout);
            print_json(&crate::jobject! {"project" => project});
        }
        Command::Top {
            limit,
            project,
            human,
        } => {
            let project = default_project(layout, project);
            let rows = search::search(layout, "", "published", *limit, &project, None)?;
            let lines: Vec<String> = rows
                .iter()
                .map(|row| {
                    if *human {
                        crate::textops::human_index_line(row)
                    } else {
                        crate::textops::index_line(row, false)
                    }
                })
                .collect();
            print_json(&crate::jobject! {
                "lines" => Json::Array(lines.into_iter().map(Json::Str).collect()),
            });
        }
        Command::Mcp => {
            return crate::mcp::main_loop(layout);
        }
        Command::Hook { event } => {
            run_hook(layout, event);
        }
        Command::HostSetup { .. } => {
            return crate::host_setup::run_cli(command);
        }
    }
    Ok(())
}

fn run_hook(layout: &Layout, event: &str) {
    match event {
        "codex-user-prompt-submit" => hooks::codex_user_prompt_submit(layout),
        "codex-session-start" => hooks::codex_session_start(layout),
        // Legacy no-op events kept for one transition release.
        "codex-post-tool-use" | "codex-stop" | "codex-session-end" => hooks::codex_noop(),
        _ => {}
    };
}

pub fn host_setup_args(command: &Command) -> Option<crate::host_setup::HostSetupArgs> {
    let Command::HostSetup {
        action,
        home,
        app_root,
        data_home,
        codex,
        opencode,
        project,
    } = command
    else {
        return None;
    };
    Some(crate::host_setup::HostSetupArgs {
        action: action.clone(),
        home: home.clone(),
        app_root: app_root.clone(),
        data_home: data_home.clone(),
        codex: codex.clone(),
        opencode: opencode.clone(),
        project: project.clone(),
    })
}
