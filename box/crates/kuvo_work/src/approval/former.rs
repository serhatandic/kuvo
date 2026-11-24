//! The ask rules a box before 0.9.189 put into bots' conversations and their work, whatever their
//! approval. A bot in a mode now does what any conversation in that mode does, and only what you
//! set in Edit Bot holds it back: these are taken out of a conversation's rules as the box makes
//! them again, never left behind as if they were its own.

use std::sync::LazyLock;

use super::Rule;

/// Programs asked whatever their arguments.
const PROGRAMS: &[&str] = &[
    "useradd",
    "adduser",
    "sysadminctl",
    "dscl",
    "mail",
    "mailx",
    "sendmail",
    "mutt",
    "msmtp",
    "swaks",
];

const PUBLISH: &[&str] = &[
    "git push*",
    "npm publish*",
    "pnpm publish*",
    "yarn publish*",
    "yarn npm publish*",
    "bun publish*",
    "cargo publish*",
    "gem push*",
    "twine upload*",
    "poetry publish*",
    "swift package-registry publish*",
    "vercel*",
    "npx vercel*",
    "netlify deploy*",
    "npx netlify*",
    "wrangler deploy*",
    "wrangler publish*",
    "wrangler pages deploy*",
    "npx wrangler*",
    "firebase deploy*",
    "fly deploy*",
    "flyctl deploy*",
    "railway up*",
    "render deploy*",
    "serverless deploy*",
    "sls deploy*",
    "docker push*",
    "podman push*",
    "kubectl apply*",
    "helm upgrade*",
    "helm install*",
    "terraform apply*",
    "tofu apply*",
    "pulumi up*",
    "eas submit*",
    "eas build*--auto-submit*",
    "fastlane*",
    "xcrun altool*",
    "gh release*",
    "gh pr merge*",
    "gh workflow run*",
];

const ALWAYS: &[&str] = &[
    "git push *--force*",
    "git push * -f*",
    "git push * +*",
    "git push *--delete*",
    "git push * -d *",
    "git push * :*",
    "git push *--mirror*",
    "gh issue create*",
    "gh issue comment*",
    "gh pr create*",
    "gh pr comment*",
    "gh pr review*",
    "gh gist create*",
    "gh release create*",
    "gh repo delete*",
    "gh repo edit*",
    "gh api *-X *",
    "gh api *--method*",
    "gh api * -f *",
    "gh api * -F *",
    "gh api *--field*",
    "stripe *",
    "npx stripe *",
    "bunx stripe *",
    "*://api.stripe.com*",
    "*://api.paypal.com*",
    "*://hooks.slack.com*",
    "*://slack.com/api/chat.*",
    "*://discord.com/api/webhooks*",
    "*://discordapp.com/api/webhooks*",
    "*://api.telegram.org*",
    "*://api.twilio.com*",
    "*://api.sendgrid.com*",
    "*://api.mailgun.net*",
    "*://api.resend.com*",
    "*://gmail.googleapis.com*",
];

const FORMER_PROGRAMS: &[&str] = &[
    "rm",
    "rmdir",
    "unlink",
    "shred",
    "srm",
    "truncate",
    "dd",
    "mkfs",
    "wipefs",
    "chmod",
    "chown",
    "chgrp",
    "chflags",
    "setfacl",
    "chattr",
    "sudo",
    "su",
    "doas",
    "visudo",
    "pkexec",
    "ssh-keygen",
    "ssh-add",
    "security",
    "op",
    "bw",
    "gpg",
    "keyring",
    "git-credential",
];

const FORMER_ASKS: &[&str] = &[
    "* xargs rm*",
    "*-c *rm *",
    "git config *",
    "git -c *",
    "git -C *",
    "git --git-dir*",
    "git remote add*",
    "git remote set-url*",
    "git remote remove*",
    "git remote rm*",
    "git clean *",
    "git reset --hard*",
    "git reset * --hard*",
    "git branch -D*",
    "git branch * -D*",
    "gh secret *",
    "gh auth token*",
    "gh auth login*",
    "gh auth refresh*",
    "gh auth setup-git*",
    "find * -delete*",
    "find * -exec rm*",
    "*.ssh/*",
    "*.aws/credentials*",
    "*.netrc*",
    "*.git-credentials*",
];

const FORMER_ALLOWS: &[&str] = &[
    "git config --get*",
    "git config --list*",
    "git config -l*",
    "git config --get-all*",
    "git config --get-regexp*",
    "git config get *",
    "git config list",
    "git config list -*",
];

const FORMER_EDITS: &[&str] = &[
    "*.env",
    "*.env.*",
    "*/.ssh/*",
    "*/.gitconfig",
    "*/.gitmodules",
    "*/.git/config",
    "*/.netrc",
    "*/.npmrc",
    "*/.pypirc",
    "*/.git-credentials",
    "*/.aws/credentials",
    "*/.docker/config.json",
    "*/.kube/config",
];

fn programs(names: &'static [&'static str]) -> impl Iterator<Item = Rule> {
    names.iter().flat_map(|program| {
        [
            (*program).to_owned(),
            format!("{program} *"),
            format!("*/{program} *"),
        ]
        .map(|resource| Rule::new("shell", &resource, "ask"))
    })
}

static FORMER: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let shell = |resources: &'static [&'static str], effect: &'static str| {
        resources
            .iter()
            .map(move |resource| Rule::new("shell", resource, effect))
    };
    programs(PROGRAMS)
        .chain(shell(ALWAYS, "ask"))
        .chain(shell(PUBLISH, "ask"))
        .chain(programs(FORMER_PROGRAMS))
        .chain(shell(FORMER_ASKS, "ask"))
        .chain(shell(FORMER_ALLOWS, "allow"))
        .chain(
            FORMER_EDITS
                .iter()
                .map(|resource| Rule::new("edit", resource, "ask")),
        )
        .chain([Rule::new("edit", "*.env.example", "allow")])
        .collect()
});

/// `rules` without what a box before put in them for a bot, in their own order: but for what
/// `kept` (Settings › Permissions) has itself.
pub fn without_former_asks(rules: &[Rule], kept: &[Rule]) -> Vec<Rule> {
    rules
        .iter()
        .filter(|rule| !FORMER.contains(rule) || kept.contains(rule))
        .cloned()
        .collect()
}

/// Whether `rules` still carry any of it, other than what `kept` has too.
pub fn has_former_asks(rules: &[Rule], kept: &[Rule]) -> bool {
    rules
        .iter()
        .any(|rule| FORMER.contains(rule) && !kept.contains(rule))
}
