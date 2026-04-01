use std::{
    fmt::{self, Debug},
    ops::Not,
    process::Command,
    str::FromStr,
    sync::LazyLock,
};

use anyhow::{Context, Result, anyhow};
use derive_more::{Deref, DerefMut, FromStr};

use itertools::Itertools;
use regex::Regex;
use semver::Version;
use serde::Deserialize;

use crate::tasks::compliance::ReleaseChannel;

pub(crate) trait Subcommand {
    type ParsedOutput: FromStr;

    fn args(&self) -> impl IntoIterator<Item = String>;
}

#[derive(Deref, DerefMut)]
pub(crate) struct GitCommand<G: Subcommand> {
    #[deref]
    #[deref_mut]
    subcommand: G,
}

impl<G: Subcommand> GitCommand<G> {
    #[must_use]
    pub fn run(subcommand: G) -> Result<G::ParsedOutput> {
        Self { subcommand }.run_impl()
    }

    fn run_impl(self) -> Result<G::ParsedOutput> {
        let command_output = Command::new("git")
            .args(self.subcommand.args())
            .output()
            .context("Failed to spawn command")?;

        String::from_utf8(command_output.stdout)
            .map_err(|_| anyhow!("Invalid UTF8"))
            .and_then(|s| {
                G::ParsedOutput::from_str(s.trim())
                    .map_err(|_| anyhow!("Failed to parse from string"))
            })
    }
}

#[derive(Deref, Debug, Clone)]
pub(crate) struct VersionTag(pub(crate) String);

impl VersionTag {
    pub(crate) fn new(version: &Version, channel: ReleaseChannel) -> Self {
        VersionTag(format!(
            "v{version}{channel_suffix}",
            version = version,
            channel_suffix = channel.tag_suffix()
        ))
    }
}

#[derive(Debug, Deref, FromStr, PartialEq, Eq, Hash, Deserialize)]
pub(crate) struct CommitSha(pub(crate) String);

// pub(crate) struct CommitForTag(pub(crate) VersionTag);

// impl Subcommand for CommitForTag {
//     type ParsedOutput = CommitSha;

//     fn args(&self) -> impl IntoIterator<Item = String> {
//         ["rev-list", "-n", "1", self.0.as_ref()].map(ToOwned::to_owned)
//     }
// }

#[derive(Debug)]
pub(crate) struct CommitDetails {
    sha: CommitSha,
    author: Committer,
    title: String,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Committer {
    name: String,
    email: String,
}

impl Committer {
    pub(crate) fn new(name: &str, email: &str) -> Self {
        Self {
            name: name.to_owned(),
            email: email.to_owned(),
        }
    }
}

impl fmt::Display for Committer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.name, self.email)
    }
}

impl CommitDetails {
    const BODY_DELIMITER: &str = "|body-delimiter|";
    const COMMIT_DELIMITER: &str = "|commit-delimiter|";
    const FIELD_DELIMITER: &str = "|field-delimiter|";
    const FORMAT_STRING: &str = "%H|field-delimiter|%an|field-delimiter|%ae|field-delimiter|%s|body-delimiter|%b|commit-delimiter|";

    fn parse(line: &str, body: &str) -> Result<Self, anyhow::Error> {
        let Some([sha, author_name, author_email, title]) =
            line.splitn(4, Self::FIELD_DELIMITER).collect_array()
        else {
            return Err(anyhow!("Failed to parse commit fields from input {line}"));
        };

        Ok(CommitDetails {
            sha: CommitSha(sha.to_owned()),
            author: Committer::new(author_name, author_email),
            title: title.to_owned(),
            body: body.to_owned(),
        })
    }

    pub(crate) fn pr_number(&self) -> Option<u64> {
        // Since we use squash merge, all commit titles end with the '(#12345)' pattern.
        // While we could strictly speaking index into this directly, go for a slightly
        // less prone approach to errors
        const PATTERN: &str = " (#";
        self.title
            .rfind(PATTERN)
            .and_then(|location| {
                self.title[location..]
                    .find(')')
                    .map(|relative_end| location + PATTERN.len()..location + relative_end)
            })
            .and_then(|range| self.title[range].parse().ok())
    }

    pub(crate) fn co_authors(&self) -> Option<Vec<Committer>> {
        static CO_AUTHOR_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"Co-authored-by: (.+) <(.+)>").unwrap());

        let mut co_authors = Vec::new();

        for cap in CO_AUTHOR_REGEX.captures_iter(&self.body.as_ref()) {
            let Some((name, email)) = cap
                .get(1)
                .map(|m| m.as_str())
                .zip(cap.get(2).map(|m| m.as_str()))
            else {
                continue;
            };
            co_authors.push(Committer::new(name, email));
        }

        co_authors.is_empty().not().then_some(co_authors)
    }

    pub(crate) fn author(&self) -> &Committer {
        &self.author
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) fn sha(&self) -> &CommitSha {
        &self.sha
    }
}

#[derive(Debug, Deref, Default, DerefMut)]
pub(crate) struct CommitList(Vec<CommitDetails>);

impl IntoIterator for CommitList {
    type IntoIter = std::vec::IntoIter<CommitDetails>;
    type Item = CommitDetails;

    fn into_iter(self) -> std::vec::IntoIter<Self::Item> {
        self.0.into_iter()
    }
}

impl FromStr for CommitList {
    type Err = anyhow::Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Ok(CommitList(
            input
                .split(CommitDetails::COMMIT_DELIMITER)
                .filter(|commit_details| !commit_details.is_empty())
                .map(|commit_details| {
                    let (line, body) = commit_details
                        .trim()
                        .split_once(CommitDetails::BODY_DELIMITER)
                        .expect("Missing body delimiter");
                    CommitDetails::parse(line, body).expect("Parsing from the output should suceed")
                })
                .collect(),
        ))
    }
}

pub(crate) struct CommitsFromVersionToHead(pub(crate) VersionTag);

impl Subcommand for CommitsFromVersionToHead {
    type ParsedOutput = CommitList;

    fn args(&self) -> impl IntoIterator<Item = String> {
        [
            "log".to_string(),
            format!("--pretty=format:{}", CommitDetails::FORMAT_STRING),
            format!("{}..HEAD", self.0.as_str()),
        ]
    }
}

pub(crate) struct Checkout(pub(crate) String);

impl Checkout {
    pub(crate) fn previous_branch() -> Self {
        Self("-".to_owned())
    }
}

impl Subcommand for Checkout {
    type ParsedOutput = String;

    fn args(&self) -> impl IntoIterator<Item = String> {
        ["checkout", self.0.as_str()].map(ToOwned::to_owned)
    }
}
