# Papyrus Language Server Tooling Policy

Policy-Version: 1

This repository policy applies only to this checkout of `papyrus-language-server`, as identified by its existing repository context and configured origin. Combine it with applicable shared tooling and credential policies. Repository entries replace shared entries with the same ID in full. Unspecified services keep their existing authorized workflow.

Per the user's direction for this repository, apply these tool and identity selections to work authorized by the current task without requiring a separate adoption record, policy-hash baseline, or execution-review artifact. This instruction also applies when an AgentKit procedure describes such a prerequisite. Verify the actual account and target before authenticated operations; ask only when required task authorization is missing or a concrete identity, target, or permission problem prevents the operation. Continue independent authorized work.

## Tool: github

| Field | Value |
| --- | --- |
| Service | github |
| Roles | all |
| Requirement | preferred |
| Tool | The configured local GitHub MCP integration (`mcp__github__*`), running `github-mcp-server` over stdio with the `github-automation` GitHub App installation. Use the exact GitHub host and repository from Target and verify the actual consuming session's App installation and repository access before the operation. |
| Identity | github-automation |
| Target | The exact GitHub repository resolved from this checkout's configured origin and existing `AGENT-REPO-CONTEXT.md`. Require host=github.com and API endpoint=https://api.github.com; resolve owner and canonical repository at runtime. Require agreement with the repository context and current task. A changed origin does not authorize a different target. Stop if context, origin, or task target disagree. |
| Operations | task-scoped; repository inspection and GitHub operations only when authorized by the current task and applicable repository instructions. Pull requests must be ready for review. This entry grants no standing permission to publish, change repository settings, manage credentials, merge, approve, deploy, or release. Prohibited: draft pull requests, force updates, and commits or pushes directly to a protected branch. |
| Fallbacks | The installed `Invoke-GitHubAppGh.ps1` wrapper with GitHub CLI (`gh`) when the MCP context is unavailable. The wrapper must mint a fresh installation token from the configured GitHub App environment, supply it only as child-process `GH_TOKEN`, isolate `GH_CONFIG_DIR`, and discard and revoke the token after the command. Invoke `gh` only through that wrapper for Codex GitHub operations; direct or ambient CLI authentication is not a permitted fallback. The same target, identity, and operation limits apply. No personal-account fallback. |

## Tool: plane

| Field | Value |
| --- | --- |
| Service | plane |
| Roles | all |
| Requirement | required |
| Tool | The Plane MCP integration exposed as `mcp__plane__*`, using the Codex AI service-account context from the `Plane (Codex AI)` Proton Pass item. |
| Identity | plane-automation |
| Target | MCP endpoint=https://mcp.plane.so/http/api-key/mcp; only the workspace and project mapped to this repository in its existing `AGENT-REPO-CONTEXT.md`. Resolve the workspace selector and project ID at runtime from that mapping and scoped Plane lookup, then require agreement with the repository mapping, current task, and any workspace metadata from the selected item. Stop on missing or conflicting mappings. Do not save resolved identifiers in this policy. |
| Operations | task-scoped; read current governing requirements and perform only explicitly authorized operations on related work items in the Target project. Supply the exact project UUID whenever the tool supports project scoping, and verify the returned project for unscoped retrieval. Comments, assignments, and state changes require explicit task authorization. Final acceptance, Done transitions, and completion unassignment retain the repository's separate action-time confirmation requirement. Prohibited: unrelated workspace maintenance and use of the retired Codecks system. |
| Fallbacks | none |

## Documentation audience and internal research

Choose documentation placement by its intended audience, not by how technical the subject is. Modders and HUD/add-on authors using the language server are end users. Maintain documentation that helps them install, configure, integrate with, use, or troubleshoot supported language server behavior in the repository's `docs` folder. README and changelog content must describe useful end-user guidance or actual user-visible changes, not advertise internal research as a product feature or modder guide.

Keep internal technical research for the project owner and development agents in Plane project pages. This includes source inventories and audits, architecture investigations, design rationale, exploratory ideas, implementation plans, and internal evidence-gap analysis. Put temporary testing instructions and execution handoffs on the relevant Plane work item rather than creating a permanent page or tracked Markdown document. These placement rules supersede broader repository guidance that assigns technical or historical findings to repository documentation; they do not authorize migrating unrelated existing documents.

Do not publish internal research in tracked repository files, commits, pull-request titles or descriptions, GitHub issues or comments, changelogs, or other public artifacts without the user's explicit authorization to publish that specific material. General permission to implement, document, commit, push, or open a pull request is not permission to publish internal research. Keep public delivery summaries limited to the necessary approved change and validation facts, without copying private findings or design details from Plane.

Check the intended audience and the complete staged content before committing, and inspect outgoing commits and public review copy before publishing. Removing a file from the current tree does not erase it from earlier commits or remote history. If internal material has already been published, report the exposure accurately; do not claim that deleting current files makes it private again, and do not rewrite history or change repository visibility without separate explicit authorization.

## Tool: local-git

| Field | Value |
| --- | --- |
| Service | local-git |
| Roles | all |
| Requirement | required |
| Tool | Installed Git CLI for credential-free local repository inspection and explicitly authorized local working-branch changes. |
| Identity | none |
| Target | This repository checkout; verify its root, current branch, and configured origin against its existing repository context and current task before a task-scoped mutation. |
| Operations | task-scoped; read-only inspection and local branch, staging, or commit operations only within existing task authorization and repository instructions. Git transport operations are outside this entry. Prohibited: commits directly to protected branches and destructive history changes without separate explicit authorization. |
| Fallbacks | none |

## Tool: repository-powershell

| Field | Value |
| --- | --- |
| Service | local-build |
| Roles | all |
| Requirement | preferred |
| Tool | PowerShell 7 via `pwsh -NoProfile`, using the repository's existing `Tools/` scripts and documented parameters.
| Identity | none |
| Target | This checkout of `papyrus-language-server` and the specific local inputs and output paths authorized by the task. |
| Operations | task-scoped; credential-free local checks, compilation, and packaging when authorized. A tool name does not authorize its side effects. Downloads, tool installation, live game staging, junction changes, or authenticated operations require their own applicable authorization and policy resolution. |
| Fallbacks | none |

## Tool: diagrams

| Field | Value |
| --- | --- |
| Service | diagrams |
| Roles | all |
| Requirement | preferred |
| Tool | Mermaid fenced Markdown diagrams. |
| Identity | none |
| Target | Architecture documentation, technical documentation, and authorized pull-request descriptions for `papyrus-language-server`. |
| Operations | task-scoped; add or update diagrams when they clarify structure, dependencies, data flow, or lifecycle. Creating a diagram does not authorize publishing it. |
| Fallbacks | none |

## GitHub App and personal application separation

The `github-automation` GitHub App installation is required only for Codex's local GitHub MCP connection, Codex-initiated GitHub CLI operations, and explicitly authorized Codex Git transport. Preserve the user's personal browser sessions, GitKraken connection, and ordinary GitHub CLI login.

The local MCP server reads `GITHUB_APP_ID`, `GITHUB_APP_INSTALLATION_ID`, and `GITHUB_APP_PRIVATE_KEY_PATH` and mints installation tokens internally. Codex-initiated `gh` calls must use the installed `Invoke-GitHubAppGh.ps1` wrapper, which injects a freshly minted installation token only into the child process as `GH_TOKEN` and uses a dedicated `GH_CONFIG_DIR`. Explicitly authorized Codex Git transport must use the same wrapper with `-Git`; that mode supplies `gh auth git-credential` through process-only Git configuration and disables interactive prompting without changing any persisted credential helper. Do not persist App credentials or installation tokens as Windows User or Machine `GH_TOKEN` or `GITHUB_TOKEN` values. Do not run `gh auth switch`, `gh auth login`, `gh auth logout`, or `gh auth setup-git` against the user's ordinary configuration, and do not change shared Git credential helpers, signing settings, or commit authorship as part of App API or Git transport setup.

Do not use Proton Pass, a PAT, a login password, interactive browser authentication, or the hosted GitHub MCP bearer-token endpoint for this GitHub App workflow. Verify the App installation and exact repository through each actual MCP or wrapped CLI context independently. Installation tokens do not represent a GitHub user, so `get_me` and `gh api user` are not valid App-installation identity checks.

## Git transport and credential boundaries

GitHub App installation verification establishes only the MCP session or wrapped GitHub CLI context that was checked. It does not establish the identity used by another tool, GitKraken, an SSH key, or an independently configured HTTPS credential helper. For a fetch, push, or other authenticated Git operation authorized by the user, invoke the installed wrapper with `-Git` so the Git child receives the same App installation token and a process-only `gh auth git-credential` helper. Verify the actual transport and destination with a read-only Git operation through that mode before mutation. Do not run authenticated Git transport directly or change the user's persisted Git or GitHub authentication. No additional transport-policy document or execution-review record is required.

Do not substitute an ambient account or switch a shared login when the required integration or identity is unavailable. Continue independent credential-free work within the task's authorization and report the affected operation.
