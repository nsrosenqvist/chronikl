//! Prose-pass integration: real classified commits → mock provider →
//! assert the prompt carried the right voice + entries, and the
//! returned Markdown ended up in the final output.

use chronikl::audit::AuditSink;
use chronikl::models::{
    Classification, ClassificationSource, Classified, ClassifiedCommit, Commit, MergeStyle, PrInfo,
    ReleaseKind, Section, VersionBump,
};
use chronikl::prose::{self, ProseRequest};
use chronikl::providers::NotesProvider;
use chronikl::providers::mock::MockProvider;
use chronikl::voice::Voice;

fn entry(
    sha: &str,
    subject: &str,
    section: Section,
    source: ClassificationSource,
    confidence: f32,
    pr: Option<PrInfo>,
) -> ClassifiedCommit {
    ClassifiedCommit {
        commit: Commit {
            sha: sha.to_string(),
            short_sha: sha[..7.min(sha.len())].to_string(),
            author_name: "ada".into(),
            author_email: "ada@x".into(),
            author_date: "2026-01-01T00:00:00+00:00".into(),
            parents: vec!["p".into()],
            subject: subject.into(),
            body: String::new(),
            files: vec![],
            pr_id: pr.as_ref().map(|p| p.number),
            conventional: None,
            breaking: section == Section::Breaking,
        },
        pr,
        classification: Classification {
            section,
            summary: subject.into(),
            source,
            confidence,
        },
    }
}

#[tokio::test]
async fn prose_pass_returns_model_markdown_and_records_audit() {
    let classified = Classified(vec![
        entry(
            "aaa",
            "Drop legacy v1 API",
            Section::Breaking,
            ClassificationSource::Conventional,
            1.0,
            None,
        ),
        entry(
            "bbb",
            "Add SSO login",
            Section::Features,
            ClassificationSource::BatchedLlm,
            0.9,
            Some(PrInfo {
                number: 42,
                title: "Auth: SSO support".into(),
                body: String::new(),
                labels: vec!["enhancement".into()],
                author: Some("ada".into()),
                merged_at: None,
                url: None,
            }),
        ),
    ]);

    let voice = Voice {
        system_prompt: "Write release notes like a brisk professional.".into(),
        is_custom: true,
        bundled_name: None,
    };

    let canned = "### Breaking Changes\n\n- Remove legacy v1 API.\n\n### Features\n\n- Add SSO login support (#42).\n";
    let audit = AuditSink::new();
    audit.enable();
    let provider = MockProvider::returning(canned).with_audit_sink(audit.clone());

    let md = prose::run(
        ProseRequest {
            voice: &voice,
            extra_instructions: Some("Mention deploy region."),
            inline_prompt: Some("Cite PR numbers in parentheses."),
            classified: &classified,
            merge_style: MergeStyle::Rebase,
            release_kind: &ReleaseKind::Stable,
            version_bump: VersionBump::Unknown,
            version_scheme: None,
            from_ref: Some("v0.1.0"),
            to_ref: "HEAD",
        },
        &provider as &dyn NotesProvider,
        &audit,
    )
    .await
    .unwrap();

    assert_eq!(md, canned.trim());

    let calls = provider.captured_calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.label, "prose");

    // System prompt = voice + extras + inline, in that order.
    assert!(call.system_prompt.starts_with("Write release notes like"));
    assert!(call.system_prompt.contains("Mention deploy region."));
    assert!(
        call.system_prompt
            .contains("Cite PR numbers in parentheses.")
    );
    let i_voice = call.system_prompt.find("Write release notes").unwrap();
    let i_extra = call.system_prompt.find("Mention deploy").unwrap();
    let i_inline = call.system_prompt.find("Cite PR numbers").unwrap();
    assert!(i_voice < i_extra);
    assert!(i_extra < i_inline);

    // User prompt has the structured entries the model was asked to format.
    assert!(call.user_prompt.contains("v0.1.0..HEAD"));
    assert!(call.user_prompt.contains("rebase"));
    assert!(call.user_prompt.contains("## Breaking Changes"));
    assert!(call.user_prompt.contains("Drop legacy v1 API"));
    assert!(call.user_prompt.contains("## Features"));
    assert!(call.user_prompt.contains("(#42)"));
    assert!(call.user_prompt.contains("PR #42: Auth: SSO support"));

    // Audit captured the prose call tagged with both SHAs.
    let document = audit.finalize(classified, md);
    let prose_call = document
        .llm_calls
        .iter()
        .find(|c| c.label == "prose")
        .unwrap();
    assert_eq!(prose_call.commit_shas.len(), 2);
}

#[tokio::test]
async fn prose_pass_strips_markdown_code_fence_wrapper() {
    let classified = Classified(vec![entry(
        "aaa",
        "Add x",
        Section::Features,
        ClassificationSource::BatchedLlm,
        0.9,
        None,
    )]);
    let voice = Voice {
        system_prompt: "x".into(),
        is_custom: false,
        bundled_name: Some("terse"),
    };
    let canned = "```markdown\n### Features\n\n- Add x.\n```";
    let provider = MockProvider::returning(canned);
    let audit = AuditSink::new();

    let md = prose::run(
        ProseRequest {
            voice: &voice,
            extra_instructions: None,
            inline_prompt: None,
            classified: &classified,
            merge_style: MergeStyle::Rebase,
            release_kind: &ReleaseKind::Stable,
            version_bump: VersionBump::Unknown,
            version_scheme: None,
            from_ref: None,
            to_ref: "HEAD",
        },
        &provider as &dyn NotesProvider,
        &audit,
    )
    .await
    .unwrap();

    assert!(!md.contains("```"));
    assert!(md.starts_with("### Features"));
}

#[tokio::test]
async fn prose_pass_propagates_provider_error() {
    let classified = Classified(vec![entry(
        "a",
        "x",
        Section::Other,
        ClassificationSource::Default,
        0.5,
        None,
    )]);
    let voice = Voice {
        system_prompt: "x".into(),
        is_custom: false,
        bundled_name: Some("terse"),
    };
    let provider = MockProvider::failing("simulated 500");
    let audit = AuditSink::new();

    let err = prose::run(
        ProseRequest {
            voice: &voice,
            extra_instructions: None,
            inline_prompt: None,
            classified: &classified,
            merge_style: MergeStyle::Rebase,
            release_kind: &ReleaseKind::Stable,
            version_bump: VersionBump::Unknown,
            version_scheme: None,
            from_ref: None,
            to_ref: "HEAD",
        },
        &provider as &dyn NotesProvider,
        &audit,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("simulated 500"));
}
