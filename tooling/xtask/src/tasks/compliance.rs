use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use crate::tasks::compliance::{
    checks::Reporter,
    git::{Checkout, CommitsFromVersionToHead, GetVersionTags, GitCommand, VersionTag},
    github::GitHubClient,
    report::ReportReviewSummary,
};

mod checks;
mod git;
mod github;
mod report;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub(crate) enum ReleaseChannel {
    Stable,
    Preview,
}

impl ReleaseChannel {
    pub(crate) fn tag_suffix(&self) -> &'static str {
        match self {
            ReleaseChannel::Stable => "",
            ReleaseChannel::Preview => "-pre",
        }
    }
}

#[derive(Parser)]
pub struct ComplianceArgs {
    #[arg(value_parser = VersionTag::parse)]
    // The version to be on the lookout for
    pub(crate) version_tag: VersionTag,
    #[arg(long)]
    // The markdown file to write the compliance report to
    report_path: PathBuf,
    #[arg(long)]
    // An optional branch to use instead of the determined version branch
    branch: Option<String>,
}

impl ComplianceArgs {
    pub(crate) fn version_tag(&self) -> &VersionTag {
        &self.version_tag
    }

    fn version_branch(&self) -> String {
        self.branch.clone().unwrap_or_else(|| {
            format!(
                "v{major}.{minor}.x",
                major = self.version_tag().version().major,
                minor = self.version_tag().version().minor
            )
        })
    }
}

async fn check_compliance_impl(args: ComplianceArgs) -> Result<()> {
    let in_workflow_context = std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true");
    let tag = args.version_tag();

    let previous_version = GitCommand::run(GetVersionTags)?
        .sorted()
        .find_previous_version(&tag)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Could not find previous version for tag {tag}",
                tag = tag.to_string()
            )
        })?;

    if !in_workflow_context {
        GitCommand::run(Checkout(args.version_branch()))?;
    }

    println!("Checking compliance for version {}", tag.version());

    let commits = GitCommand::run(CommitsFromVersionToHead(previous_version))?;

    println!("Found {} commits to check", commits.len());

    let app_id = std::env::var("GITHUB_APP_ID").context("Missing GITHUB_APP_ID")?;
    let key = std::env::var("GITHUB_APP_KEY").context("Missing GITHUB_APP_KEY")?;

    let client = GitHubClient::for_app(
        app_id.parse().context("Failed to parse app ID as int")?,
        key.as_ref(),
    )
    .await?;

    println!("Initialized GitHub client for app ID {app_id}");

    let report = Reporter::new(commits, client).generate_report().await?;

    let summary = report.summary();
    let report_path = args.report_path.with_extension("md");

    report.write_markdown(&report_path)?;

    println!("Wrote compliance report to {}", report_path.display());

    if !in_workflow_context {
        GitCommand::run(Checkout::previous_branch())?;
    }

    match summary.review_summary() {
        ReportReviewSummary::MissingReviews => Err(anyhow::anyhow!(
            "Compliance check failed, found {} commits not reviewed",
            summary.not_reviewed
        )),
        ReportReviewSummary::MissingReviewsWithErrors => Err(anyhow::anyhow!(
            "Compliance check failed with {} unreviewed commits and {} other issues",
            summary.not_reviewed,
            summary.errors
        )),
        ReportReviewSummary::NoIssuesFound => {
            println!("No issues found, compliance check passed.");
            Ok(())
        }
    }
}

pub fn check_compliance(args: ComplianceArgs) -> Result<()> {
    tokio::runtime::Runtime::new()
        .context("Failed to create tokio runtime")
        .and_then(|handle| handle.block_on(check_compliance_impl(args)))
}
