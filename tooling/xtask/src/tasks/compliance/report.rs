use std::{mem, ops::Not, time::Duration};

use octocrab::models::pulls::{Comment, Review, ReviewState};

use crate::tasks::compliance::{
    git::{CommitDetails, CommitList},
    github::{CommitAuthor, GitHubClient, GithubLogin},
};

const ZED_ZIPPY_COMMENT_APPROVAL_PATTERN: &str = "@zed-zippy approved";

#[derive(Debug)]
pub(crate) enum ReviewSuccess {
    CoAuthored(Vec<CommitAuthor>),
    PullRequestReviewed(Vec<Review>),
    ApprovingComment(Vec<Comment>),
}

#[derive(Debug)]
pub(crate) enum ReviewFailure {
    // todo: We could still query the GitHub API here to search for one
    NoPullRequestFound,
    Unreviewed,
    Other(anyhow::Error),
}

pub(crate) type ReviewResult = Result<ReviewSuccess, ReviewFailure>;

impl<E: Into<anyhow::Error>> From<E> for ReviewFailure {
    fn from(err: E) -> Self {
        Self::Other(anyhow::anyhow!(err))
    }
}

#[derive(Debug)]
pub(crate) struct Report {
    reports: Vec<(CommitDetails, ReviewResult)>,
}

impl Report {
    pub fn new() -> Self {
        Self {
            reports: Vec::new(),
        }
    }

    pub fn add(&mut self, commit: CommitDetails, result: ReviewResult) {
        self.reports.push((commit, result));
    }
}

pub(crate) struct Reporter {
    commits: CommitList,
    github_client: GitHubClient,
}

impl Reporter {
    pub fn new(commits: CommitList, github_client: GitHubClient) -> Self {
        Self {
            commits,
            github_client,
        }
    }

    async fn check_commit(
        &mut self,
        commit: &CommitDetails,
    ) -> Result<ReviewSuccess, ReviewFailure> {
        // Check co-authors first
        if commit.co_authors().is_some()
            && let Some(commit_authors) = self
                .github_client
                .get_commit_co_authors([commit.sha()])
                .await?
                .get(commit.sha())
                .and_then(|authors| authors.co_authors())
        {
            let mut org_co_authors = Vec::new();
            for co_author in commit_authors {
                if let Some(github_login) = co_author.user()
                    && self
                        .github_client
                        .check_org_membership(github_login)
                        .await?
                {
                    org_co_authors.push(co_author.clone());
                }
            }

            if org_co_authors.is_empty().not() {
                return Ok(ReviewSuccess::CoAuthored(org_co_authors));
            }
        }

        // Check PR after
        let Some(pr_number) = commit.pr_number() else {
            return Err(ReviewFailure::NoPullRequestFound);
        };

        let other_comments = self
            .github_client
            .get_pr_reviews(pr_number)
            .await?
            .take_items();

        if !other_comments.is_empty() {
            let mut org_approving_reviews = Vec::new();
            for review in other_comments {
                if let Some(github_login) = review.user.as_ref()
                    && review
                        .state
                        .is_some_and(|state| state == ReviewState::Approved)
                    && self
                        .github_client
                        .check_org_membership(&GithubLogin::new(github_login.login.clone()))
                        .await?
                {
                    org_approving_reviews.push(review);
                }
            }

            if org_approving_reviews.is_empty().not() {
                return Ok(ReviewSuccess::PullRequestReviewed(org_approving_reviews));
            }
        }

        let other_comments = self
            .github_client
            .get_pr_comments(pr_number)
            .await?
            .take_items();

        if !other_comments.is_empty() {
            let mut org_approving_comments = Vec::new();
            for comment in other_comments {
                if let Some(github_login) = comment.user.as_ref()
                    && comment.body.contains(ZED_ZIPPY_COMMENT_APPROVAL_PATTERN)
                    && self
                        .github_client
                        .check_org_membership(&GithubLogin::new(github_login.login.clone()))
                        .await?
                {
                    org_approving_comments.push(comment);
                }
            }

            if org_approving_comments.is_empty().not() {
                return Ok(ReviewSuccess::ApprovingComment(org_approving_comments));
            }
        }

        Err(ReviewFailure::Unreviewed)
    }

    pub(crate) async fn generate_report(&mut self) -> anyhow::Result<Report> {
        let mut report = Report::new();

        for commit in mem::take(&mut self.commits).into_iter() {
            println!("Checking commit {:?}", commit.sha().as_str());

            let review_result = self.check_commit(&commit).await;

            report.add(commit, review_result);

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(report)
    }
}
