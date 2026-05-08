use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use clap::Parser;
use colored::Colorize;

use chronikl::audit::{AuditSink, ConfigSnapshot, RangeSnapshot};
use chronikl::cache::{ClassificationCache, NullCache, default_cache_root, disk::DiskCache};
use chronikl::cli::{
    CacheCommand, Cli, Command, DebugCommand, GenerateArgs, LicenseCommand, RangeArgs,
};
use chronikl::config::Config;
use chronikl::constants::{BUILD_DATE, FULL_VERSION, GIT_SHA, TARGET};
use chronikl::enrichment;
use chronikl::env::Env;
use chronikl::git::{self, Range, merge_style};
use chronikl::ladder::{tier0, tier1, tier2, tier3};
use chronikl::license::{self, ExpiryStatus};
use chronikl::models::Classified;
use chronikl::output::{self, RenderOptions};
use chronikl::progress::{HeaderInfo, LicenseStatus, Progress, SummaryInfo};
use chronikl::prose::{self, ProseRequest};
use chronikl::providers::{NotesProvider, rig::RigProvider};
use chronikl::telemetry::{HeartbeatPayload, TierCounts};
use chronikl::voice;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("{} {err:#}", "Error:".red().bold());
        process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => generate(cli.generate).await,
        Some(Command::Version) => {
            print_version();
            Ok(())
        }
        Some(Command::Validate { file }) => validate_voice(&file),
        Some(Command::Cache(cmd)) => cache_command(cmd),
        Some(Command::License(cmd)) => license_command(cmd),
        Some(Command::Update { force }) => chronikl::update::run_update(force)
            .await
            .map_err(Into::into),
        Some(Command::Debug(DebugCommand::Commits { range, repo })) => {
            debug_commits(range, repo).await
        }
        Some(Command::Debug(DebugCommand::MergeStyle { range, repo })) => {
            debug_merge_style(range, repo).await
        }
        Some(Command::Debug(DebugCommand::Config { repo })) => debug_config(repo),
        Some(Command::Debug(DebugCommand::Classify { range, repo })) => {
            debug_classify(range, repo).await
        }
        Some(Command::Debug(DebugCommand::Prompts {
            range,
            repo,
            voice,
            prompt,
            rich_context,
            no_pr_enrichment,
        })) => debug_prompts(range, repo, voice, prompt, rich_context, no_pr_enrichment).await,
    }
}

fn print_version() {
    println!("chronikl {FULL_VERSION}");
    println!("  commit: {GIT_SHA}");
    println!("  built:  {BUILD_DATE}");
    println!("  target: {TARGET}");
}

fn validate_voice(file: &Path) -> anyhow::Result<()> {
    let v = voice::load_from_file(file)?;
    println!("{} {}", "OK".green().bold(), file.display());
    println!("  body: {} bytes", v.system_prompt.len());
    Ok(())
}

fn license_command(cmd: LicenseCommand) -> anyhow::Result<()> {
    match cmd {
        LicenseCommand::Activate { key } => {
            let raw = match key {
                Some(k) => k,
                None => {
                    use std::io::BufRead;
                    let stdin = std::io::stdin();
                    stdin
                        .lock()
                        .lines()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("no license key provided"))?
                        .map_err(anyhow::Error::from)?
                }
            };
            let claims = license::verify_license_key(raw.trim())?;
            let path = license::write_to_disk(raw.trim())?;
            println!("{} {}", "OK".green().bold(), path.display());
            println!(
                "  customer: {} ({})",
                claims.customer_name, claims.customer_id
            );
            println!("  issued:   {}", claims.issued_at);
            println!("  expires:  {}", claims.expires_at);
            match license::check_expiry(&claims)? {
                ExpiryStatus::Valid => println!("  status:   {}", "valid".green()),
                ExpiryStatus::ExpiringSoon { days } => {
                    println!("  status:   {} ({days} days)", "expiring soon".yellow())
                }
                ExpiryStatus::Expired => println!("  status:   {}", "expired".red()),
            }
        }
        LicenseCommand::Status => match license::resolve_key() {
            None => println!("{} no license configured", "info:".dimmed()),
            Some(key) => match license::verify_license_key(&key) {
                Ok(claims) => {
                    println!(
                        "customer: {} ({})",
                        claims.customer_name, claims.customer_id
                    );
                    println!("issued:   {}", claims.issued_at);
                    println!("expires:  {}", claims.expires_at);
                    match license::check_expiry(&claims)? {
                        ExpiryStatus::Valid => println!("status:   {}", "valid".green()),
                        ExpiryStatus::ExpiringSoon { days } => {
                            println!("status:   {} ({days} days)", "expiring soon".yellow())
                        }
                        ExpiryStatus::Expired => println!("status:   {}", "expired".red()),
                    }
                }
                Err(e) => {
                    eprintln!("{} {e}", "Error:".red().bold());
                    return Err(anyhow::anyhow!("invalid license"));
                }
            },
        },
        LicenseCommand::Deactivate => {
            let removed = license::remove_from_disk()?;
            if removed {
                println!("{} license removed", "OK".green().bold());
            } else {
                println!("{} no license file to remove", "info:".dimmed());
            }
        }
    }
    Ok(())
}

fn cache_command(cmd: CacheCommand) -> anyhow::Result<()> {
    let repo = std::env::current_dir().expect("cwd available");
    let cache = DiskCache::new(default_cache_root(&repo));
    match cmd {
        CacheCommand::Path => {
            if let Some(p) = cache.root() {
                println!("{}", p.display());
            } else {
                println!("(no cache root)");
            }
        }
        CacheCommand::Stats => {
            let s = cache.stats();
            println!("entries: {}", s.entries);
            println!("bytes:   {}", s.bytes);
            if let Some(p) = cache.root() {
                println!("path:    {}", p.display());
            }
        }
        CacheCommand::Clear => {
            let n = cache.clear()?;
            println!(
                "{} cleared {n} cache entr{} at {}",
                "OK".green().bold(),
                if n == 1 { "y" } else { "ies" },
                cache
                    .root()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(unknown)".into())
            );
        }
    }
    Ok(())
}

/// Aggregate counters populated during a run, used to build the
/// telemetry heartbeat at the end (regardless of success/failure).
struct RunStats {
    commit_count: usize,
    tier_counts: TierCounts,
    provider: Option<String>,
    model: Option<String>,
    merge_style: String,
    prerelease: bool,
    version_bump: chronikl::models::VersionBump,
    version_scheme: Option<chronikl::models::VersionScheme>,
    custom_voice: bool,
    agent_used: bool,
    pr_enrichment: bool,
    audit_log: bool,
}

impl Default for RunStats {
    fn default() -> Self {
        Self {
            commit_count: 0,
            tier_counts: TierCounts::default(),
            provider: None,
            model: None,
            merge_style: String::new(),
            prerelease: false,
            version_bump: chronikl::models::VersionBump::Unknown,
            version_scheme: None,
            custom_voice: false,
            agent_used: false,
            pr_enrichment: false,
            audit_log: false,
        }
    }
}

async fn generate(args: GenerateArgs) -> anyhow::Result<()> {
    let mut stats = RunStats::default();
    let result = generate_inner(args.clone(), &mut stats).await;

    // Heartbeat: fire-and-forget at the end. Honour --no-telemetry, the
    // env var, and the TOML setting. We re-read config here cheaply —
    // it's local TOML + env, no network. If config load itself errors
    // we just skip telemetry (no-op, can't tell what to send).
    let telemetry_enabled = telemetry_decision(&args).unwrap_or(false);
    if telemetry_enabled {
        // License presence in the heartbeat is informational. The
        // license module short-circuits expired keys to None so this
        // tracks the "active" population, not just "anyone has ever
        // entered a key".
        let licensed = license::current_claims().is_some();
        let payload = HeartbeatPayload::from_release(
            stats.commit_count,
            stats.tier_counts,
            stats.provider,
            stats.model,
            if stats.merge_style.is_empty() {
                "unknown".to_string()
            } else {
                stats.merge_style
            },
            stats.prerelease,
            stats.version_bump.as_str(),
            stats.version_scheme.map(|s| s.as_str().to_string()),
            stats.custom_voice,
            stats.agent_used,
            stats.pr_enrichment,
            stats.audit_log,
            licensed,
            result.is_ok(),
        );
        // Fire-and-forget — but with a short bounded await so the POST
        // has a chance to complete before `#[tokio::main]` drops the
        // runtime (which cancels still-pending spawned tasks).
        let handle = chronikl::telemetry::send_heartbeat(payload);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
    }

    result
}

/// Decide whether telemetry should fire based on flag/env/config.
/// Errors loading config → telemetry off (safe default).
fn telemetry_decision(args: &GenerateArgs) -> anyhow::Result<bool> {
    if args.no_telemetry {
        return Ok(false);
    }
    let repo = args
        .repo
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd available"));
    let env = Env::real();
    let home = dirs::config_dir();
    let cfg = Config::load(&env, &repo, home.as_deref())?;
    Ok(cfg.telemetry.enabled)
}

async fn generate_inner(args: GenerateArgs, stats: &mut RunStats) -> anyhow::Result<()> {
    let progress = Progress::new(true);

    let repo = args
        .repo
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd available"));
    let env = Env::real();
    let home = dirs::config_dir();
    let cfg = Config::load(&env, &repo, home.as_deref())?;

    stats.audit_log = args.audit_log.is_some();
    stats.provider = cfg.provider.name.clone();
    stats.model = cfg.provider.model.clone();

    let spec = args.range.to_spec();
    let range = git::resolve_range(&repo, &spec).await?;
    let commits = git::log(&repo, &range).await?;
    stats.commit_count = commits.len();

    let detected_style = merge_style::detect(&commits);
    stats.merge_style = detected_style.to_string();
    let release_kind = git::detect_release_kind(&repo, &range.to).await?;
    stats.prerelease = release_kind.is_prerelease();
    let (version_bump, version_scheme) =
        git::detect_version_bump(&repo, range.from.as_deref(), &range.to).await?;
    stats.version_bump = version_bump;
    stats.version_scheme = version_scheme;

    // Voice resolution — CLI `--voice` (name or path) wins over TOML
    // `[voice].path` which wins over TOML `[voice].profile`; otherwise
    // bundled `terse`. Flagged in the audit and the run header.
    let voice_obj = voice::resolve(
        args.voice.as_deref(),
        cfg.voice.path.as_deref(),
        cfg.voice.profile.as_deref(),
    )?;
    stats.custom_voice = voice_obj.is_custom;

    let release_label = match &release_kind {
        chronikl::models::ReleaseKind::Stable => "stable".to_string(),
        chronikl::models::ReleaseKind::Prerelease { label } => format!("prerelease ({label})"),
        chronikl::models::ReleaseKind::Untagged => "untagged".to_string(),
    };
    let bump_label = match version_scheme {
        Some(scheme) => format!("{} ({})", version_bump.as_str(), scheme.as_str()),
        None => version_bump.as_str().to_string(),
    };
    let range_label = format!(
        "{}..{} ({} commits, {detected_style})",
        range.from.as_deref().unwrap_or("(initial)"),
        range.to,
        commits.len(),
    );
    // Resolve license claims for the banner. Only treat the claim as
    // active when it verifies AND hasn't expired — a stored-but-expired
    // key falls back to the free-tier banner so users notice their key
    // needs renewal.
    let license_claims = license::current_claims().filter(|c| {
        matches!(
            license::check_expiry(c),
            Ok(license::ExpiryStatus::Valid | license::ExpiryStatus::ExpiringSoon { .. })
        )
    });
    let license_status = match &license_claims {
        Some(c) => LicenseStatus::Licensed {
            name: &c.customer_name,
        },
        None => LicenseStatus::Free,
    };
    // Voice header label: bundled profile name when bundled, else the
    // file name of the custom voice. Custom-voice path priority mirrors
    // the resolver: CLI `--voice` (since `bundled_name == None` here
    // implies CLI was a path, not a profile name) before `[voice].path`.
    let voice_label: String = if let Some(name) = voice_obj.bundled_name {
        name.to_string()
    } else {
        let path = args
            .voice
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| cfg.voice.path.clone());
        path.as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "custom".to_string())
    };
    progress.header(HeaderInfo {
        version: FULL_VERSION,
        range: &range_label,
        release: &release_label,
        version_bump: &bump_label,
        provider: cfg.provider.name.as_deref(),
        model: cfg.provider.model.as_deref(),
        voice: &voice_label,
        license: license_status,
    });

    // ── Stage: PR enrichment ────────────────────────────────────────
    let mut enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let (enricher, enrich_note) =
        enrichment::platform::detect(&repo, &env, args.no_pr_enrichment).await?;
    stats.pr_enrichment = enricher.name() == "github";
    if args.no_pr_enrichment {
        progress.stage_skip("PR enrichment", "disabled (--no-pr-enrichment)");
    } else if enricher.name() == "no-op" {
        progress.stage_skip("PR enrichment", enrich_note);
    } else {
        let started = progress.stage_start();
        match enricher.enrich(&mut enriched).await {
            Ok(outcome) if outcome.failed > 0 => progress.stage_warn(
                "PR enrichment",
                format!(
                    "{} ok / {} failed (first: {})",
                    outcome.enriched,
                    outcome.failed,
                    outcome.first_error.as_deref().unwrap_or("?")
                ),
            ),
            Ok(outcome) => progress.stage_done(
                "PR enrichment",
                format!(
                    "{} of {} commit(s) enriched",
                    outcome.enriched,
                    commits.len()
                ),
                started,
            ),
            Err(e) => progress.stage_warn("PR enrichment", e),
        }
    }

    // Audit setup. Recording is always on (records buffered in memory
    // even when --audit-log isn't requested) so the summary block can
    // surface call counts and token totals. The JSON file is only
    // written when the user opted in via flag/env.
    let audit = AuditSink::new();
    audit.enable();
    audit.set_chronikl_version(FULL_VERSION.to_string());
    audit.set_voice_is_custom(voice_obj.is_custom);
    audit.set_voice_name(voice_obj.bundled_name.map(str::to_string));
    audit.set_config(snapshot_config(&cfg));
    audit.set_range(snapshot_range(
        &range,
        &commits,
        detected_style,
        &release_kind,
        version_bump,
        version_scheme,
    ));

    // ── Stage: Tier 0 ───────────────────────────────────────────────
    let started = progress.stage_start();
    let mut classified = tier0::classify(&enriched);
    let confident_t0 = classified
        .iter()
        .filter(|c| (c.classification.confidence - 1.0).abs() < f32::EPSILON)
        .count();
    progress.stage_done(
        "Tier 0 deterministic",
        format!("{confident_t0}/{} placed", classified.len()),
        started,
    );
    stats.tier_counts.tier0 = classified.len();

    // ── Stage: Cache populate ──────────────────────────────────────
    let cache: Box<dyn ClassificationCache> = if args.no_cache {
        Box::new(NullCache)
    } else {
        Box::new(DiskCache::new(default_cache_root(&repo)))
    };
    let model_for_cache = cfg.provider.model.clone().unwrap_or_default();
    if !args.no_llm && !model_for_cache.is_empty() {
        let hits = cache.populate(&mut classified, &model_for_cache);
        stats.tier_counts.cache_hits = hits;
        if hits > 0 {
            progress.info(format!("cache hit on {hits} commit(s)"));
        }
    }

    // Tier 1+2 + prose pass: only if a provider is configured and the user
    // didn't opt out. Falls back to the deterministic renderer otherwise.
    let prose_markdown = if should_run_llm(&args, &cfg) {
        run_llm_pipeline(LlmPipelineCtx {
            args: &args,
            cfg: &cfg,
            repo: &repo,
            voice_obj: &voice_obj,
            classified: &mut classified,
            detected_style,
            range: &range,
            release_kind: &release_kind,
            version_bump,
            version_scheme,
            audit: &audit,
            cache: cache.as_ref(),
            model_for_cache: &model_for_cache,
            stats,
            progress: &progress,
        })
        .await?
    } else {
        progress.stage_skip(
            "LLM pipeline",
            "no provider configured (--no-llm or missing key)",
        );
        None
    };

    // Compare-URL footer. When `origin` (or `upstream`) points at a
    // recognized forge — GitHub, GitLab, Bitbucket Cloud, Gitea/Forgejo,
    // Codeberg — append a `**Full Changelog**` line so readers can open
    // the diff with one click. Each forge has its own URL convention
    // (Bitbucket reverses operands, GitLab uses `/-/`, etc.); the
    // [`Forge`] strategy hides those differences. Independent of
    // `--no-pr-enrichment`, which only governs PR data fetching.
    let compare_footer: Option<String> = chronikl::enrichment::remote::detect_forge(&repo)
        .await
        .map(|forge| {
            format!(
                "**Full Changelog**: {}",
                forge.compare_url(range.from.as_deref(), &range.to)
            )
        });

    let markdown = match prose_markdown {
        Some(md) => append_footer(md, compare_footer.as_deref()),
        None => {
            let mut opts = RenderOptions::for_release(&release_kind);
            if opts.footer.is_none() {
                opts.footer = compare_footer.clone();
            }
            output::render(&classified, &opts)
        }
    };

    if let Some(path) = &args.output {
        std::fs::write(path, markdown.as_bytes())?;
    } else {
        use std::io::{IsTerminal, Write};
        // When stdout shares the terminal with stderr, the Markdown
        // would otherwise land mid-stream between stage rows and the
        // summary block. Bracket it with a labeled divider on stderr
        // (never on stdout — that would contaminate piped output).
        let stdout_tty = std::io::stdout().is_terminal();
        if stdout_tty {
            progress.divider_labeled("release notes");
            std::io::stderr().flush().ok();
        }
        print!("{markdown}");
        if !markdown.ends_with('\n') {
            println!();
        }
        std::io::stdout().flush().ok();
    }

    // Write the audit log (if requested) before rendering the summary
    // so the summary can name it as a trailing reference line.
    let audit_log_str = if let Some(audit_path) = &args.audit_log {
        let document = audit.finalize(classified, markdown.clone());
        let json = serde_json::to_string_pretty(&document)?;
        std::fs::write(audit_path, json)?;
        Some(audit_path.display().to_string())
    } else {
        None
    };

    let output_path_str = args.output.as_ref().map(|p| p.display().to_string());
    let (call_count, token_totals) = audit.totals();
    progress.summary(SummaryInfo {
        output_path: output_path_str.as_deref(),
        output_bytes: markdown.len(),
        commits: stats.commit_count,
        llm_calls: call_count,
        tokens: token_totals,
        elapsed: progress.elapsed(),
        audit_log_path: audit_log_str.as_deref(),
    });

    Ok(())
}

fn snapshot_config(cfg: &Config) -> ConfigSnapshot {
    ConfigSnapshot {
        provider: cfg.provider.name.clone(),
        model: cfg.provider.model.clone(),
        agent_fallback: cfg.ladder.agent_fallback,
        max_diff_tokens: cfg.ladder.max_diff_tokens,
        confidence_threshold: cfg.ladder.confidence_threshold,
        batch_size: cfg.ladder.batch_size,
        merge_style_override: None,
    }
}

/// Append a footer line to a Markdown document, separated by a blank
/// line. Idempotent when the footer is already present at the end —
/// guards against the prose model occasionally including a changelog
/// link itself.
fn append_footer(markdown: String, footer: Option<&str>) -> String {
    let Some(footer) = footer else {
        return markdown;
    };
    let trimmed = markdown.trim_end();
    if trimmed.contains(footer) {
        // Model already produced an identical line; don't duplicate.
        return format!("{trimmed}\n");
    }
    format!("{trimmed}\n\n{footer}\n")
}

/// Whether the LLM pipeline should run for this invocation. Returns
/// `false` for `--no-llm`, missing provider config, or missing API key
/// (Ollama is exempt — it runs locally without a key).
fn should_run_llm(args: &GenerateArgs, cfg: &Config) -> bool {
    !args.no_llm
        && cfg.provider.name.is_some()
        && (cfg.provider.api_key.is_some() || cfg.provider.name.as_deref() == Some("ollama"))
}

/// Inputs for [`run_llm_pipeline`]. Bundling them keeps the function
/// signature manageable.
struct LlmPipelineCtx<'a> {
    args: &'a GenerateArgs,
    cfg: &'a Config,
    repo: &'a Path,
    voice_obj: &'a chronikl::voice::Voice,
    classified: &'a mut Classified,
    detected_style: chronikl::models::MergeStyle,
    range: &'a Range,
    release_kind: &'a chronikl::models::ReleaseKind,
    version_bump: chronikl::models::VersionBump,
    version_scheme: Option<chronikl::models::VersionScheme>,
    audit: &'a AuditSink,
    cache: &'a dyn ClassificationCache,
    model_for_cache: &'a str,
    stats: &'a mut RunStats,
    progress: &'a Progress,
}

/// Run Tier 1, 2 (always), Tier 3 (opt-in), then the prose pass.
/// Returns the prose-pass Markdown when it succeeds; `None` to fall
/// back to the deterministic renderer.
async fn run_llm_pipeline(ctx: LlmPipelineCtx<'_>) -> anyhow::Result<Option<String>> {
    let LlmPipelineCtx {
        args,
        cfg,
        repo,
        voice_obj,
        classified,
        detected_style,
        range,
        release_kind,
        version_bump,
        version_scheme,
        audit,
        cache,
        model_for_cache,
        stats,
        progress,
    } = ctx;

    let rig = match RigProvider::new(cfg.provider.clone()) {
        Ok(p) => Arc::new(p.with_audit_sink(audit.clone())),
        Err(e) => {
            progress.stage_warn("LLM pipeline", format!("provider not usable: {e}"));
            return Ok(None);
        }
    };
    let provider: Arc<dyn NotesProvider> = rig.clone();

    // ── Stage: Tier 1 ──────────────────────────────────────────────
    let pending_t1 = classified
        .iter()
        .filter(|c| c.classification.confidence < 1.0)
        .count();
    if pending_t1 == 0 {
        progress.stage_skip("Tier 1 batched LLM", "no commits below confidence 1.0");
    } else {
        let started = progress.stage_start();
        let updated_t1 = tier1::classify(
            classified,
            provider.as_ref(),
            audit,
            cfg.ladder.batch_size,
            1.0,
            tier1::default_system_prompt(),
        )
        .await?;
        stats.tier_counts.tier1 = updated_t1;
        let batches = pending_t1.div_ceil(cfg.ladder.batch_size.max(1));
        progress.stage_done(
            "Tier 1 batched LLM",
            format!(
                "{updated_t1} of {pending_t1} commit(s) in {batches} batch{}",
                if batches == 1 { "" } else { "es" }
            ),
            started,
        );
    }

    // ── Stage: Tier 2 ──────────────────────────────────────────────
    let pending_t2 = classified
        .iter()
        .filter(|c| c.classification.confidence < cfg.ladder.confidence_threshold)
        .count();
    if pending_t2 == 0 {
        progress.stage_skip(
            "Tier 2 per-commit",
            format!("none below threshold {}", cfg.ladder.confidence_threshold),
        );
    } else {
        let started = progress.stage_start();
        let updated_t2 = tier2::classify(
            classified,
            repo,
            provider.as_ref(),
            audit,
            cfg.ladder.max_diff_tokens,
            cfg.ladder.confidence_threshold,
            tier2::default_system_prompt(),
        )
        .await?;
        stats.tier_counts.tier2 = updated_t2;
        progress.stage_done(
            "Tier 2 per-commit",
            format!("{updated_t2} of {pending_t2} commit(s) (with diff)"),
            started,
        );
    }

    // ── Stage: Tier 3 (opt-in) ─────────────────────────────────────
    if args.agent || cfg.ladder.agent_fallback {
        let pending_t3 = classified
            .iter()
            .filter(|c| c.classification.confidence < cfg.ladder.confidence_threshold)
            .count();
        if pending_t3 == 0 {
            progress.stage_skip("Tier 3 agentic", "nothing left to classify");
        } else {
            let started = progress.stage_start();
            let updated_t3 = tier3::classify(
                classified,
                repo,
                rig.as_ref(),
                audit,
                cfg.ladder.max_diff_tokens,
                cfg.ladder.confidence_threshold,
                tier3::default_max_turns(),
                tier3::default_system_prompt(),
            )
            .await?;
            stats.tier_counts.tier3 = updated_t3;
            stats.agent_used = updated_t3 > 0;
            progress.stage_done(
                "Tier 3 agentic",
                format!("{updated_t3} of {pending_t3} commit(s)"),
                started,
            );
        }
    }

    // Persist newly LLM-derived classifications to the cache so the
    // next run skips them. Done *before* the prose pass so a prose-pass
    // failure still saves ladder progress.
    if !model_for_cache.is_empty() {
        let written = cache.persist_llm_results(classified, model_for_cache);
        if written > 0 {
            progress.info(format!("cached {written} new classification(s)"));
        }
    }

    // ── Stage: Prose pass ──────────────────────────────────────────
    if classified.is_empty() {
        progress.stage_skip("Prose pass", "no commits to summarize");
        return Ok(None);
    }
    let started = progress.stage_start();
    let request = ProseRequest {
        voice: voice_obj,
        extra_instructions: cfg.voice.extra_instructions.as_deref(),
        inline_prompt: args.prompt.as_deref(),
        classified,
        merge_style: detected_style,
        release_kind,
        version_bump,
        version_scheme,
        from_ref: range.from.as_deref(),
        to_ref: &range.to,
        rich_context: args.rich_context || cfg.voice.rich_context,
    };
    match prose::run(request, provider.as_ref(), audit).await {
        Ok(md) => {
            progress.stage_done(
                "Prose pass",
                chronikl::progress::format_bytes(md.len()),
                started,
            );
            Ok(Some(md))
        }
        Err(e) => {
            progress.stage_warn(
                "Prose pass",
                format!("failed; falling back to deterministic render: {e}"),
            );
            Ok(None)
        }
    }
}

fn snapshot_range(
    range: &Range,
    commits: &[chronikl::models::Commit],
    style: chronikl::models::MergeStyle,
    release_kind: &chronikl::models::ReleaseKind,
    version_bump: chronikl::models::VersionBump,
    version_scheme: Option<chronikl::models::VersionScheme>,
) -> RangeSnapshot {
    RangeSnapshot {
        from: range.from.clone(),
        to: range.to.clone(),
        commit_count: commits.len(),
        detected_merge_style: style.to_string(),
        prerelease: release_kind.is_prerelease(),
        release_label: release_kind.label().map(|s| s.to_string()),
        version_bump: version_bump.as_str().to_string(),
        version_scheme: version_scheme.map(|s| s.as_str().to_string()),
    }
}

async fn debug_commits(range: RangeArgs, repo: Option<PathBuf>) -> anyhow::Result<()> {
    let commits = load_commits(range, repo).await?;
    println!("{}", serde_json::to_string_pretty(&commits)?);
    Ok(())
}

async fn debug_merge_style(range: RangeArgs, repo: Option<PathBuf>) -> anyhow::Result<()> {
    let commits = load_commits(range, repo).await?;
    let style = merge_style::detect(&commits);
    println!("{style} ({} commits)", commits.len());
    Ok(())
}

async fn debug_classify(range: RangeArgs, repo: Option<PathBuf>) -> anyhow::Result<()> {
    let commits = load_commits(range, repo).await?;
    let enriched = chronikl::models::EnrichedCommit::from_commits(commits);
    let classified: Classified = tier0::classify(&enriched);
    println!("{}", serde_json::to_string_pretty(&classified)?);
    Ok(())
}

/// Dump the prompts each tier + the prose pass *would* construct for
/// the given range. Doesn't call any LLM. Honours the configured voice
/// + inline prompt. Helpful when debugging voice tweaks or prompt
///   regressions.
async fn debug_prompts(
    range: RangeArgs,
    repo: Option<PathBuf>,
    voice_arg: Option<String>,
    inline_prompt: Option<String>,
    rich_context_arg: bool,
    no_pr_enrichment: bool,
) -> anyhow::Result<()> {
    use chronikl::ladder::{tier1, tier2, tier3};
    use chronikl::prose::prompts as prose_prompts;

    let repo_path = repo.unwrap_or_else(|| std::env::current_dir().expect("cwd available"));
    let env = Env::real();
    let home = dirs::config_dir();
    let cfg = Config::load(&env, &repo_path, home.as_deref())?;

    let spec = range.to_spec();
    let resolved = git::resolve_range(&repo_path, &spec).await?;
    let commits = git::log(&repo_path, &resolved).await?;

    // Same enrichment path as `generate` so the dumped prompts match
    // what would actually be sent (PR titles/labels included if so).
    let mut enriched = chronikl::models::EnrichedCommit::from_commits(commits.clone());
    let (enricher, _info) =
        enrichment::platform::detect(&repo_path, &env, no_pr_enrichment).await?;
    let _ = enricher.enrich(&mut enriched).await;

    let detected_style = merge_style::detect(&commits);
    let release_kind = git::detect_release_kind(&repo_path, &resolved.to).await?;
    let (version_bump, version_scheme) =
        git::detect_version_bump(&repo_path, resolved.from.as_deref(), &resolved.to).await?;

    // Voice resolution mirrors generate(): CLI flag (name or path)
    // wins, then TOML path, then TOML profile, then bundled `terse`.
    let voice_obj = voice::resolve(
        voice_arg.as_deref(),
        cfg.voice.path.as_deref(),
        cfg.voice.profile.as_deref(),
    )?;

    let classified = tier0::classify(&enriched);

    println!("# Prompts that would be sent");
    println!();
    println!(
        "Range: `{}` (commits: {})",
        resolved.as_arg(),
        commits.len()
    );
    println!("Merge style: {detected_style}");
    println!(
        "Release kind: {}",
        match &release_kind {
            chronikl::models::ReleaseKind::Stable => "stable".to_string(),
            chronikl::models::ReleaseKind::Prerelease { label } => {
                format!("prerelease ({label})")
            }
            chronikl::models::ReleaseKind::Untagged => "untagged".to_string(),
        }
    );
    println!(
        "Version bump: {}{}",
        version_bump.as_str(),
        version_scheme
            .map(|s| format!(" ({})", s.as_str()))
            .unwrap_or_default()
    );
    println!();

    // ── Tier 1 ──────────────────────────────────────────────────────
    let tier1_pending: Vec<&chronikl::models::ClassifiedCommit> = classified
        .iter()
        .filter(|c| c.classification.confidence < 1.0)
        .collect();
    println!("## Tier 1 — batched LLM (would-send)");
    println!();
    if tier1_pending.is_empty() {
        println!("_No commits below conf=1.0; Tier 1 wouldn't run._");
    } else {
        println!(
            "{} commits would be sent in batches of {}.",
            tier1_pending.len(),
            cfg.ladder.batch_size
        );
        println!();
        println!("### system");
        println!("```");
        println!("{}", tier1::default_system_prompt());
        println!("```");
        println!();
        for (i, chunk) in tier1_pending
            .chunks(cfg.ladder.batch_size.max(1))
            .enumerate()
        {
            println!("### user (batch {})", i + 1);
            println!("```");
            println!("{}", tier1::debug_batch_prompt(chunk));
            println!("```");
            println!();
        }
    }

    // ── Tier 2 ──────────────────────────────────────────────────────
    println!("## Tier 2 — per-commit + diff (would-send)");
    println!();
    let tier2_pending: Vec<&chronikl::models::ClassifiedCommit> = classified
        .iter()
        .filter(|c| c.classification.confidence < cfg.ladder.confidence_threshold)
        .collect();
    if tier2_pending.is_empty() {
        println!(
            "_No commits below conf={}; Tier 2 wouldn't run._",
            cfg.ladder.confidence_threshold
        );
    } else {
        println!("### system");
        println!("```");
        println!("{}", tier2::default_system_prompt());
        println!("```");
        println!();
        for entry in &tier2_pending {
            let diff = git::commit_diff(&repo_path, &entry.commit.sha)
                .await
                .unwrap_or_default();
            let truncated = tier2::truncate_diff(&diff, cfg.ladder.max_diff_tokens);
            println!("### user (commit {})", entry.commit.short_sha);
            println!("```");
            println!("{}", tier2::debug_prompt(entry, &truncated));
            println!("```");
            println!();
        }
    }

    // ── Tier 3 ──────────────────────────────────────────────────────
    println!("## Tier 3 — agentic (would-send, initial turn only)");
    println!();
    if tier2_pending.is_empty() {
        println!("_No commits below the threshold; Tier 3 wouldn't run._");
    } else {
        println!("### system");
        println!("```");
        println!("{}", tier3::default_system_prompt());
        println!("```");
        println!();
        for entry in &tier2_pending {
            let diff = git::commit_diff(&repo_path, &entry.commit.sha)
                .await
                .unwrap_or_default();
            let truncated = tier2::truncate_diff(&diff, cfg.ladder.max_diff_tokens);
            println!("### user (commit {}, initial turn)", entry.commit.short_sha);
            println!("```");
            println!("{}", tier3::debug_prompt(entry, &truncated));
            println!("```");
            println!();
        }
    }

    // ── Prose pass ──────────────────────────────────────────────────
    println!("## Prose pass (would-send)");
    println!();
    println!("### system (voice + addenda)");
    println!("```");
    println!(
        "{}",
        prose_prompts::build_system_prompt(
            &voice_obj,
            cfg.voice.extra_instructions.as_deref(),
            inline_prompt.as_deref(),
            &release_kind,
            version_bump,
            version_scheme,
        )
    );
    println!("```");
    println!();
    println!("### user");
    println!("```");
    println!(
        "{}",
        prose_prompts::build_user_prompt(
            &classified,
            resolved.from.as_deref(),
            &resolved.to,
            detected_style,
            &release_kind,
            version_bump,
            version_scheme,
            rich_context_arg || cfg.voice.rich_context,
        )
    );
    println!("```");

    Ok(())
}

fn debug_config(repo: Option<PathBuf>) -> anyhow::Result<()> {
    let repo = repo.unwrap_or_else(|| std::env::current_dir().expect("cwd available"));
    let env = Env::real();
    let home = dirs::config_dir();
    let cfg = Config::load(&env, &repo, home.as_deref())?;
    println!("{}", serde_json::to_string_pretty(&cfg)?);
    Ok(())
}

async fn load_commits(
    range: RangeArgs,
    repo: Option<PathBuf>,
) -> anyhow::Result<Vec<chronikl::models::Commit>> {
    let repo = repo.unwrap_or_else(|| std::env::current_dir().expect("cwd available"));
    let spec = range.to_spec();
    let resolved = git::resolve_range(&repo, &spec).await?;
    let commits = git::log(&repo, &resolved).await?;
    Ok(commits)
}
