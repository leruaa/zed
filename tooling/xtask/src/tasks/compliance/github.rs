use std::{
    collections::{HashMap, HashSet},
    ops::Not,
};

use anyhow::{Context, Result};
use derive_more::Deref;
use itertools::Itertools;
use jsonwebtoken::EncodingKey;
use octocrab::{Octocrab, Page, models::pulls::Review};
use serde::Deserialize;

use crate::tasks::compliance::git::CommitSha;

const PAGE_SIZE: u8 = 64;
const ORG: &str = "zed-industries";
const REPO: &str = "zed";

pub struct GitHubClient {
    client: Octocrab,
    organization_members: Option<HashSet<String>>,
}

#[derive(Debug, PartialEq, Eq, Hash, Deserialize)]
enum Role {
    Admin,
    #[serde(rename = "User")]
    Member,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Deserialize, Clone, Deref, PartialEq, Eq)]
pub(crate) struct GithubLogin {
    login: String,
}

impl GithubLogin {
    pub(crate) fn new(login: String) -> Self {
        Self { login }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct CommitAuthor {
    name: String,
    email: String,
    user: Option<GithubLogin>,
}

impl CommitAuthor {
    pub(crate) fn user(&self) -> Option<&GithubLogin> {
        self.user.as_ref()
    }
}

impl PartialEq for CommitAuthor {
    fn eq(&self, other: &Self) -> bool {
        self.user.as_ref().zip(other.user.as_ref()).map_or_else(
            || self.email == other.email || self.name == other.name,
            |(l, r)| l == r,
        )
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitAuthors {
    #[serde(rename = "author")]
    primary_author: CommitAuthor,
    #[serde(rename = "authors")]
    co_authors: Vec<CommitAuthor>,
}

impl CommitAuthors {
    pub(crate) fn co_authors(&self) -> Option<impl Iterator<Item = &CommitAuthor>> {
        self.co_authors.is_empty().not().then(|| {
            self.co_authors
                .iter()
                .filter(|co_author| *co_author != &self.primary_author)
        })
    }
}

#[derive(Debug, Deserialize, Deref)]
pub(crate) struct AuthorsForCommits(HashMap<CommitSha, CommitAuthors>);

impl GitHubClient {
    pub async fn for_app(app_id: u64, app_private_key: &str) -> Result<Self> {
        let octocrab = Octocrab::builder()
            // TODO! check
            // .cache(InMemoryCache::new())
            .app(
                app_id.into(),
                EncodingKey::from_rsa_pem(app_private_key.as_bytes())?,
            )
            .build()?;

        let installations = octocrab
            .apps()
            .installations()
            .send()
            .await
            .context("Failed to fetch installations")?
            .take_items();

        let installation_id = installations
            .into_iter()
            .find(|installation| installation.account.login == ORG)
            .context("Could not find Zed repository in installations")?
            .id;

        octocrab
            .installation(installation_id)
            .map(Self::new)
            .map_err(Into::into)
    }

    fn new(client: Octocrab) -> Self {
        Self {
            client,
            organization_members: None,
        }
    }

    fn build_co_authors_query<'a>(shas: impl IntoIterator<Item = &'a CommitSha>) -> String {
        const FRAGMENT: &str = r#"
            ... on Commit {
                author {
                    name
                    email
                    user { login }
                }
                authors(first: 10) {
                    nodes {
                        name
                        email
                        user { login }
                    }
                }
            }
        "#;

        let objects: String = shas
            .into_iter()
            .map(|commit_sha| {
                format!(
                    "commit{sha}: object(oid: \"{sha}\") {{ {FRAGMENT} }}",
                    sha = **commit_sha
                )
            })
            .join("\n");

        format!("{{  repository(owner: \"{ORG}\", name: \"{REPO}\") {{ {objects}  }} }}")
            .replace("\n", "")
    }

    pub(crate) async fn get_commit_co_authors(
        &self,
        commit_shas: impl IntoIterator<Item = &CommitSha>,
    ) -> Result<AuthorsForCommits> {
        let query = Self::build_co_authors_query(commit_shas);

        let query = serde_json::json!({ "query": query });

        let mut response = self.graphql::<serde_json::Value>(&query).await?;

        // TODO speaks for itself
        response
            .get_mut("data")
            .and_then(|data| data.get_mut("repository"))
            .and_then(|repo| repo.as_object_mut())
            .ok_or_else(|| anyhow::anyhow!("Unexpected response format!"))
            .and_then(|commit_data| {
                let mut response_map = serde_json::Map::with_capacity(commit_data.len());

                for (key, value) in commit_data.iter_mut() {
                    let key_without_prefix = key.strip_prefix("commit").unwrap_or(key);
                    if let Some(authors) = value.get_mut("authors") {
                        if let Some(nodes) = authors.get("nodes") {
                            *authors = nodes.clone();
                        }
                    }

                    response_map.insert(key_without_prefix.to_owned(), value.clone());
                }

                serde_json::from_value(serde_json::Value::Object(response_map))
                    .context("Failed to deserialize commit authors")
            })
    }

    pub(crate) async fn graphql<R: octocrab::FromResponse>(
        &self,
        query: &serde_json::Value,
    ) -> octocrab::Result<R> {
        self.client.graphql(query).await
    }

    pub async fn get_pr_reviews(&self, pr_number: u64) -> octocrab::Result<Page<Review>> {
        // TODO! pagination
        self.client
            .pulls(ORG, REPO)
            .list_reviews(pr_number)
            .per_page(PAGE_SIZE)
            .send()
            .await
    }

    pub async fn get_pr_comments(
        &self,
        pr_number: u64,
    ) -> octocrab::Result<Page<octocrab::models::pulls::Comment>> {
        self.client
            .pulls(ORG, REPO)
            .list_comments(Some(pr_number))
            .per_page(PAGE_SIZE)
            .send()
            .await
    }

    pub async fn check_org_membership(&mut self, login: &GithubLogin) -> octocrab::Result<bool> {
        let members = match self.organization_members.as_ref() {
            Some(members) => members,
            None => {
                let mut fetched_members = HashSet::new();
                let mut page: u32 = 1;
                loop {
                    let mut res = self
                        .client
                        .orgs(ORG)
                        .list_members()
                        .page(page)
                        .per_page(PAGE_SIZE)
                        .send()
                        .await?;

                    fetched_members.extend(res.take_items().into_iter().map(|member| member.login));

                    if res.incomplete_results == Some(true) || res.next.is_some() {
                        page += 1;
                    } else {
                        break;
                    }
                }

                log::info!("Found {} organization members", fetched_members.len());

                self.organization_members.insert(fetched_members)
            }
        };

        Ok(members.contains(login.as_str()))
    }
}
