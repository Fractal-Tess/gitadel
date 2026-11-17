pub(super) const TAGS: &[(&str, &str)] = &[
    (
        "Instance",
        "Version, changelog, public instance settings, and first-run setup.",
    ),
    (
        "Authentication",
        "Sign-in, sign-up, password reset, passkeys, and single sign-on.",
    ),
    (
        "Account",
        "The signed-in user: profile, emails, keys, tokens, and preferences.",
    ),
    ("Organizations", "Organizations and their members."),
    (
        "Namespaces",
        "Users and organizations as repository owners: profile, activity, and pins.",
    ),
    (
        "Repositories",
        "Creating, listing, configuring, archiving, and deleting repositories.",
    ),
    (
        "Repository content",
        "Trees, files, commits, history, references, and rendered content.",
    ),
    ("Collaborators", "Repository collaborators and deploy keys."),
    (
        "Branch protection",
        "Rules that restrict pushes to matching branches.",
    ),
    ("Issues", "Issues, comments, labels, and attachments."),
    ("Releases", "Releases and their assets."),
    (
        "Commit statuses",
        "Statuses reported for commits, Gitea-compatible.",
    ),
    ("Webhooks", "Repository webhooks and their deliveries."),
    ("Mirrors", "Pull mirrors and the credentials they use."),
    ("Imports", "Importing repositories from other forges."),
    (
        "Deployments",
        "Dokploy connections and repository deployments.",
    ),
    (
        "Actions",
        "Workflows, runs, runners, secrets, and variables.",
    ),
    (
        "Administration",
        "Instance administration: users, authentication, email, and integrity.",
    ),
    ("Storage", "Storage targets, domains, and migrations."),
    ("Backups", "Backup providers, schedules, and restores."),
    (
        "Gitea compatibility",
        "Endpoints shaped like Gitea's API, for existing clients.",
    ),
];
