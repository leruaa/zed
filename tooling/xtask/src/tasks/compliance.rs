use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use semver::Version;

use crate::tasks::compliance::{
    checks::Reporter,
    git::{Checkout, CommitsFromVersionToHead, GitCommand, VersionTag},
    github::GitHubClient,
};

mod checks;
mod git;
mod github;
mod report;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
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
    #[arg(value_parser = Version::parse)]
    // The version to be on the lookout for
    pub(crate) version: Version,
    #[arg(value_enum, default_value_t = ReleaseChannel::Stable)]
    // The release channel to check compliance for
    release_channel: ReleaseChannel,
    #[arg(long)]
    // The markdown file to write the compliance report to
    report_path: PathBuf,
}

impl ComplianceArgs {
    #[allow(dead_code)]
    pub(crate) fn version_tag(&self) -> VersionTag {
        VersionTag::new(&self.version, self.release_channel)
    }

    pub(crate) fn previous_version_tag(&self) -> VersionTag {
        // TODO make this more robust
        let previous_version = Version::new(
            self.version.major,
            self.version.minor - 1,
            self.version.patch,
        );

        VersionTag::new(&previous_version, self.release_channel)
    }

    fn version_branch(&self) -> String {
        format!(
            "v{major}.{minor}.x",
            major = self.version.major,
            minor = self.version.minor
        )
    }
}

async fn check_compliance_impl(args: ComplianceArgs) -> Result<()> {
    let in_pr_context = std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true");
    let tag = args.previous_version_tag();

    if !in_pr_context {
        GitCommand::run(Checkout(args.version_branch()))?;
    }

    println!("Checking compliance for version: {}", tag.0);

    let commits = GitCommand::run(CommitsFromVersionToHead(tag.clone()))?;

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

    report.write_markdown(&args.report_path)?;

    println!("Wrote compliance report to {}", args.report_path.display());

    if !in_pr_context {
        GitCommand::run(Checkout::previous_branch())?;
    }

    if summary.every_commit_reviewed() {
        println!("No issues found, compliance check passed.");
        Ok(())
    } else if summary.commits_reviewed_with_errors() {
        println!("Issues found, compliance check failed.");
        Err(anyhow::anyhow!(
            "Compliance check failed with {} unreviewed commits and {} other issues",
            summary.not_reviewed,
            summary.errors
        ))
    } else {
        Err(anyhow::anyhow!(
            "Compliance check failed, found {} commits not reviewed",
            summary.not_reviewed
        ))
    }
}

pub fn check_compliance(args: ComplianceArgs) -> Result<()> {
    tokio::runtime::Runtime::new()
        .context("Failed to create tokio runtime")
        .and_then(|handle| handle.block_on(check_compliance_impl(args)))
}
