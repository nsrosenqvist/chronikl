//! What kind of file a commit touched: product code, tests or docs.
//!
//! Used to show the classifier a commit's product files before its test
//! and doc files (a feature that adds ten e2e specs and one component
//! used to look like a "tests" commit in a truncated file list), and to
//! catch a verdict of `tests` or `documentation` for a commit that also
//! changes product code.

use std::path::Path;

/// Rough kind of a changed file, from its path alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileKind {
    /// Anything that isn't recognizably a test or a doc: the product.
    Product,
    Test,
    Docs,
}

/// Directory names that hold tests.
const TEST_DIRS: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "spec",
    "specs",
    "e2e",
    "testing",
    "testdata",
    "fixtures",
    "androidTest",
];

/// Directory names that hold documentation.
const DOC_DIRS: &[&str] = &["docs", "doc", "adr", "adrs"];

/// Classify one path.
pub fn kind_of(path: &str) -> FileKind {
    let p = Path::new(path);
    let in_dir = |dirs: &[&str]| {
        p.parent()
            .into_iter()
            .flat_map(|d| d.components())
            .any(|c| dirs.contains(&c.as_os_str().to_str().unwrap_or("")))
    };
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let lower = name.to_ascii_lowercase();
    let test_name = lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("_test.")
        || lower.starts_with("test_")
        || name.ends_with("Test.kt")
        || name.ends_with("Tests.kt")
        || name.ends_with("Test.java")
        || name.ends_with("Tests.swift");
    if test_name || in_dir(TEST_DIRS) {
        return FileKind::Test;
    }
    let doc_ext = [".md", ".mdx", ".rst", ".adoc"]
        .iter()
        .any(|e| lower.ends_with(e));
    if in_dir(DOC_DIRS) || (doc_ext && !in_dir(&["src", "app", "apps", "lib", "packages"])) {
        return FileKind::Docs;
    }
    FileKind::Product
}

/// Check a classifier's section against the files: a commit that
/// changes product code isn't tests-only or docs-only, whatever the
/// model said. Returns the corrected section and a confidence cap, or
/// `None` when the verdict stands.
pub fn correct_section(
    section: crate::models::Section,
    files: &[String],
) -> Option<(crate::models::Section, f32)> {
    use crate::models::Section;
    match section {
        Section::Tests | Section::Documentation if touches_product(files) => {
            Some((Section::Other, 0.5))
        }
        _ => None,
    }
}

/// Whether any file is product code.
pub fn touches_product(files: &[String]) -> bool {
    files.iter().any(|f| kind_of(f) == FileKind::Product)
}

/// The files with product code first, then tests, then docs, each group
/// in its original order.
pub fn product_first(files: &[String]) -> Vec<&str> {
    let mut sorted: Vec<&str> = files.iter().map(String::as_str).collect();
    sorted.sort_by_key(|f| kind_of(f));
    sorted
}

/// One-line count by kind, e.g. `3 product, 10 test, 1 doc`.
pub fn summary(files: &[String]) -> String {
    let count = |k: FileKind| files.iter().filter(|f| kind_of(f) == k).count();
    format!(
        "{} product, {} test, {} doc",
        count(FileKind::Product),
        count(FileKind::Test),
        count(FileKind::Docs)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(files: &[&str]) -> Vec<String> {
        files.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recognizes_tests_docs_and_product() {
        for t in [
            "apps/web/e2e/mapUi.spec.ts",
            "apps/web/test/components/signInPage.test.ts",
            "apps/web/android/car-core/src/test/kotlin/app/montiva/car/core/CarAccessTest.kt",
            "tests/cli_render_integration.rs",
            "pkg/foo_test.go",
        ] {
            assert_eq!(kind_of(t), FileKind::Test, "{t}");
        }
        for d in [
            "docs/adr/0104-landing-page.md",
            "README.md",
            "docs/legal/privacy.md",
        ] {
            assert_eq!(kind_of(d), FileKind::Docs, "{d}");
        }
        for p in [
            "apps/web/src/presentation/components/account/SignInPage.vue",
            "apps/web/src/presentation/i18n/locales/en-US/landing.json",
            "apps/web/src/presentation/help/intro.md",
            "src/main.rs",
            "Cargo.toml",
        ] {
            assert_eq!(kind_of(p), FileKind::Product, "{p}");
        }
    }

    #[test]
    fn tests_or_docs_verdicts_on_product_changes_are_corrected() {
        use crate::models::Section;
        // Montiva 672cffa2: the landing page's FAQ text (product) plus
        // the roadmap (docs), filed as documentation by five models.
        let faq = v(&[
            "apps/web/src/presentation/i18n/locales/en-US/landing.json",
            "docs/roadmap.md",
        ]);
        assert_eq!(
            correct_section(Section::Documentation, &faq),
            Some((Section::Other, 0.5))
        );
        // Montiva 5e797907: one Kotlin test file. Tests is right.
        let car_test = v(&["apps/web/android/car-core/src/test/kotlin/CarAccessTest.kt"]);
        assert_eq!(correct_section(Section::Tests, &car_test), None);
        // Other sections are never second-guessed.
        assert_eq!(correct_section(Section::Features, &faq), None);
        // No files known (e.g. a shallow clone): trust the model.
        assert_eq!(correct_section(Section::Tests, &[]), None);
    }

    #[test]
    fn product_files_come_first() {
        // Montiva 8bf4ff12: ten e2e specs sorted before the code.
        let files = v(&[
            "apps/web/e2e/follow.spec.ts",
            "apps/web/e2e/mapUi.spec.ts",
            "docs/features/map.md",
            "apps/web/src/main.ts",
            "apps/web/src/presentation/components/map/ToolsMenu.vue",
        ]);
        assert_eq!(
            product_first(&files),
            vec![
                "apps/web/src/main.ts",
                "apps/web/src/presentation/components/map/ToolsMenu.vue",
                "apps/web/e2e/follow.spec.ts",
                "apps/web/e2e/mapUi.spec.ts",
                "docs/features/map.md",
            ]
        );
        assert_eq!(summary(&files), "2 product, 2 test, 1 doc");
        assert!(touches_product(&files));
        assert!(!touches_product(&v(&[
            "apps/web/e2e/a.spec.ts",
            "docs/x.md"
        ])));
    }
}
