# repoctl v0.4 Operations Session Review Spec

Status: Draft

Source learning: repeated operational sessions in the Cellis `universe`
monorepo, especially the `iostream.app` Cloudflare DNS-only migration,
CloudFront/Lambda Function URL authentication fix, multi-stack Pulumi
deployments, and post-deploy verification.

Related specs:

- [repoctl v0.2 Design Spec](repoctl-v0.2-design.md)
- [repoctl v0.2 Implementation Plan](repoctl-v0.2-impl-plan.md)
- [repoctl v0.3 Adoption And Monorepo Hardening Spec](repoctl-v0.3-adoption-spec.md)

## 1. Session Review

The `universe` work showed that `repoctl` is useful as a graph anchor, but it is
not yet strong enough for day-2 operational changes across DNS, CDN, IaC, shared
framework packages, and application stacks.

Observed session sequence:

1. `iostream.app` moved from Route53-hosted operational DNS to Cloudflare.
2. Cloudflare was intentionally configured as DNS-only, with AWS CloudFront
   remaining the serving CDN.
3. Core DNS/certificate infrastructure changed in `core-infra/nucleus`.
4. Operon shared infrastructure changed from Route53 alias records to
   Cloudflare DNS records.
5. Every existing Operon app/foundation Pulumi workspace needed dependency and
   lockfile updates.
6. Multiple staging stacks were deployed and verified through CloudFront.
7. A Google login callback on `slides.dev.int.iostream.app` exposed a downstream
   `InvalidSignatureException` from Lambda Function URL IAM signing.
8. The real fix required changing Lambda Function URL auth semantics, adding the
   newer two-permission Function URL public access model, avoiding a risky
   Pulumi AWS provider major upgrade, deploying three staging stacks, removing a
   temporary manual permission, and proving Pulumi previews were unchanged.
9. The final commit touched eight functional projects and one shared framework.

`repoctl graph validate` gave a useful baseline. `repoctl affected` identified
the broad project set and `core-infra` risk. However, several gaps remained:

- `repoctl affected` emitted duplicate task entries such as repeated
  `infra:check` tasks.
- For the same change, `repoctl run check --affected --dry-run` planned zero
  commands even though `repoctl affected` reported many check tasks.
- `repoctl pr summary` suggested only `repoctl graph validate` and
  `repoctl affected`, not the smallest meaningful build, Pulumi preview, deploy,
  DNS, or smoke-test sequence.
- Risk flags were path-oriented, not operationally specific. The report said
  `core-infra`, but did not distinguish DNS authority transfer, certificate
  validation, CloudFront alias routing, Cloudflare proxy policy, or production
  stack gaps.
- There was no command to ask "what stacks must be previewed and in what order
  for this shared infrastructure change?"
- There was no model for Cloudflare DNS-only records serving AWS CloudFront
  distributions, so the operator had to manually verify `proxied=false`,
  CNAMEs, and CloudFront response headers.
- There was no provider capability check. Trying to use a field only available
  in Pulumi AWS 7 caused a broad provider migration and SQS update failure before
  the safer Pulumi AWS 6 dynamic-resource fix was chosen.
- There was no session journal or handoff artifact tying together commands run,
  stacks deployed, manual hotfixes, cleanup actions, HTTP probes, and remaining
  production gaps.

## 2. Problem Statement

`repoctl` models project boundaries and affected tasks, but operational changes
need a richer plan than "these projects changed."

In a real monorepo, one change can cross:

- authoritative DNS provider;
- DNS record proxy semantics;
- certificate validation records;
- CloudFront distribution aliases and origins;
- Lambda Function URL resource policies;
- Pulumi provider schema capabilities;
- shared framework package consumers and lockfiles;
- staging and production stack inventories;
- manual cloud-console or CLI repair actions that must be reconciled back into
  IaC.

Without first-class support, agents and operators must build their own runbook
from memory. That creates avoidable risk: applying stacks out of order, upgrading
providers unnecessarily, forgetting a consumer lockfile, leaving temporary cloud
state unmanaged, or declaring a fix before smoke tests prove the request reached
the application layer.

## 3. Design Thesis

Operational work should be plan-first, evidence-backed, and reconciled.

`repoctl v0.4` should add an operations layer that:

1. turns a diff into an ordered operational plan;
2. explains stack, DNS, CDN, runtime, dependency, and verification impact;
3. records session evidence as commands run and resources observed;
4. distinguishes intended IaC state from temporary manual cloud state;
5. keeps repoctl out of the role of cloud provider replacement or deploy system.

The tool should not hide Pulumi, AWS, Cloudflare, or package managers. It should
make their required commands discoverable, ordered, reviewable, and verifiable.

## 4. Goals

- Fix affected task deduplication and ensure `repoctl run --affected --dry-run`
  can plan the same task set reported by `repoctl affected`.
- Add `repoctl ops plan` for multi-project operational diffs.
- Add stack inventory and ordered IaC preview planning across affected projects.
- Add DNS/CDN intent modeling for providers such as Cloudflare DNS-only in front
  of AWS CloudFront.
- Add provider capability diagnostics for Pulumi provider/schema gaps before
  broad dependency upgrades.
- Add operational smoke tests as manifest-declared probes.
- Add session journaling so agents can resume, summarize, and audit long
  infrastructure sessions.
- Add manual-state reconciliation checks for cloud mutations done outside IaC.
- Improve PR summaries so high-risk operational diffs include concrete
  verification commands and unresolved gaps.

## 5. Non-Goals

- Do not replace Pulumi, Terraform, AWS CLI, Cloudflare API, package managers, or
  HTTP clients.
- Do not automatically apply infrastructure changes by default.
- Do not store secret values in session journals, plans, logs, JSON output, or
  generated specs.
- Do not infer business correctness of application login flows beyond declared
  probes and observable responses.
- Do not require every repo to use Cloudflare or AWS. The model must support
  adapters.

## 6. Command Surface

### 6.1 `repoctl ops plan`

```bash
repoctl ops plan \
  --base origin/main \
  --head HEAD \
  --env staging \
  --tasks check,test,build \
  --format human
```

The plan should include:

- affected projects and workspaces;
- deduplicated task commands;
- stack preview order;
- stack apply commands, marked gated and disabled by default;
- DNS/CDN changes and verification probes;
- provider capability warnings;
- required environment variables by name only;
- smoke tests;
- manual-state reconciliation checks;
- unresolved production gaps.

JSON output should be stable enough for CI and agent handoff.

### 6.2 `repoctl ops verify`

```bash
repoctl ops verify --plan target/repoctl/ops-plan.json
```

Runs non-mutating verification stages:

- graph validation;
- task dry-runs;
- Pulumi previews;
- DNS resolution checks;
- provider capability checks;
- HTTP probes;
- manual-state drift checks.

Mutating commands must be listed but skipped unless an explicit apply mode is
requested.

### 6.3 `repoctl ops journal`

```bash
repoctl ops journal start --name iostream-cloudflare-migration
repoctl ops journal add-command -- repoctl graph validate
repoctl ops journal add-note --kind finding --message "Cloudflare records are DNS-only"
repoctl ops journal summary --format markdown
```

The journal is a local evidence file under `target/repoctl/sessions/` by default.
It records:

- command text;
- exit status;
- redacted output digest or selected evidence;
- timestamps;
- affected plan ID;
- stack names;
- resource identifiers;
- manual changes and cleanup status;
- final verification results.

It must redact values that look like tokens, API keys, cookies, authorization
headers, or decrypted secret parameters.

### 6.4 `repoctl iac preview --affected`

Existing `repoctl iac plan --affected` should grow into a useful preview router:

```bash
repoctl iac preview --affected --env staging --format human
```

For Pulumi, it should render concrete commands:

```bash
cd foundations/ligand/infra
AWS_PROFILE=iostream AWS_REGION=us-east-2 AWS_DEFAULT_REGION=us-east-2 \
  pulumi preview --stack staging
```

The command should distinguish:

- stack exists;
- stack missing;
- backend not logged in;
- provider plugin mismatch;
- state lock held;
- preview has creates/updates/deletes/replacements.

## 7. Manifest Model Additions

### 7.1 DNS Intent

Add repo-level or project-level DNS declarations:

```yaml
dns:
  zones:
    - name: iostream.app
      provider: cloudflare
      policy: dns-only
      records:
        default_proxy: false
        ttl: 300
```

For app/foundation stacks:

```yaml
dns:
  records:
    - name: slides.dev.int.iostream.app
      type: cname
      target:
        kind: cloudfront-distribution
        output: cdnDomainName
      proxied: false
      ttl: 300
```

`repoctl ops plan` should flag drift if a Cloudflare record is proxied when the
manifest says DNS-only.

### 7.2 CDN Intent

Declare expected serving layer:

```yaml
cdn:
  provider: aws-cloudfront
  aliases:
    - slides.dev.int.iostream.app
  expected_response_headers:
    - "via: *CloudFront*"
    - "x-cache: *cloudfront*"
```

This supports a direct answer to: "Cloudflare handles DNS, and CloudFront still
serves CDN, right?"

### 7.3 Runtime Probes

Projects should be able to declare smoke tests that do not leak secrets:

```yaml
ops:
  probes:
    - name: homepage
      method: HEAD
      url: https://slides.dev.int.iostream.app
      expect:
        status: 200
        headers:
          via: "*CloudFront*"
    - name: fake-auth-callback
      method: GET
      url: https://slides.dev.int.iostream.app/api/auth/google/callback?state=test&code=test
      expect:
        status: 401
        body_contains: "missing cookie header"
```

Probes should classify failures:

- DNS failure;
- TLS/certificate failure;
- CloudFront failure;
- cloud provider auth failure before application;
- application-level error.

### 7.4 Operational Dependencies

Declare cross-service runtime dependencies:

```yaml
ops:
  runtime_dependencies:
    - project: foundations.ligand
      endpoint: https://ligand.dev.int.iostream.app
      purpose: invitation verification
    - project: foundations.atp
      endpoint: https://atp.dev.int.iostream.app
      purpose: user registration and billing bootstrap
```

When `apps.golgi` login depends on `ligand` and `atp`, the operations plan
should include their stack health and probes.

## 8. Provider Capability Checks

The Lambda Function URL fix showed a common failure mode: a provider version can
lack one cloud feature, and upgrading the provider can cause unrelated resource
state churn.

Add:

```bash
repoctl provider capabilities --workspace frameworks/operon:infra-ts
```

For Pulumi TypeScript workspaces, inspect installed provider packages and schema
metadata when available. Report:

- provider package and version;
- known support for required fields;
- fields used in code but absent from local types;
- provider major upgrades introduced by `package.json` or lockfile changes;
- resources likely to receive state-only updates after provider migration.

Example diagnostic:

```text
provider.capability.missing
  workspace: frameworks.operon:infra-ts
  package: @pulumi/aws 6.83.4
  resource: aws.lambda.Permission
  field: invokedViaFunctionUrl
  advice: avoid broad provider upgrade; use an explicit compatibility adapter or upgrade all impacted stacks with previews.
```

## 9. Manual-State Reconciliation

Operational sessions sometimes require a temporary manual action to restore
service before the IaC fix is ready. The session must not end with unmanaged
state.

Add:

```bash
repoctl ops reconcile --plan target/repoctl/ops-plan.json
```

For each recorded manual action:

- show whether IaC now manages equivalent state;
- show whether temporary state still exists;
- provide cleanup commands;
- mark reconciliation complete only after verification.

Example:

```text
manual.lambda.add-permission
  function: cellis-slides-staging-fn-d181595
  statement: cellis-slides-staging-fn-url-invoke-public-manual
  status: removed
  managed_equivalent: pulumi-nodejs:dynamic:Resource cellis-slides-staging-fn-url-invoke-public
```

## 10. Affected And Task Planning Fixes

The current `universe` replay produced duplicate affected tasks and then planned
zero commands for `repoctl run check --affected --dry-run`.

Required fixes:

- task IDs in `AffectedReport.tasks` must be unique;
- task selection must preserve project/workspace/task identity;
- `repoctl run <task> --affected --dry-run` must use the same affected file set
  and task resolution path as `repoctl affected`;
- if no command is planned, output must explain why:
  - no base/head available;
  - no changed files;
  - affected projects have no matching task;
  - task exists in affected report but cannot resolve to a command;
  - manifest task is disabled for the selected environment.

Acceptance example:

```bash
repoctl affected --base HEAD~1 --head HEAD --tasks check --format json
repoctl run check --affected --base HEAD~1 --head HEAD --dry-run
```

The second command must plan exactly the deduplicated check commands from the
first command, or emit diagnostics with stable IDs.

## 11. PR Summary Improvements

For operational diffs, `repoctl pr summary` should include:

- deploy surface summary:
  - DNS provider changes;
  - CDN serving layer;
  - Lambda Function URL policy changes;
  - Pulumi provider major upgrades;
  - package lockfile fanout;
- ordered verification:
  - graph validate;
  - affected dry-run;
  - shared package checks;
  - stack previews;
  - stack applies, if already executed;
  - smoke probes;
  - final unchanged previews;
- unresolved gaps:
  - missing prod stacks;
  - missing prod DNS records;
  - skipped heavyweight gates;
  - manual-state cleanup pending.

For the reviewed `universe` change, the summary should have identified that:

- Cloudflare was authoritative DNS but not CDN;
- CloudFront still served the app;
- `ligand`, `atp`, and `slides` staging needed coordinated Function URL policy
  deployment;
- `ligand.int.iostream.app` and `slides.int.iostream.app` production DNS were
  not completed in that session;
- the final proof was HTTP 200 for downstream POST probes and unchanged Pulumi
  previews.

## 12. Data Model Sketch

```rust
pub struct OpsPlan {
    pub id: OpsPlanId,
    pub repo_root: RepoRoot,
    pub base: Option<GitRef>,
    pub head: Option<GitRef>,
    pub environments: Vec<EnvironmentName>,
    pub affected: AffectedReport,
    pub task_plan: TaskRunReport,
    pub iac: Vec<IacOperation>,
    pub dns: Vec<DnsOperation>,
    pub cdn: Vec<CdnCheck>,
    pub provider_capabilities: Vec<ProviderCapabilityReport>,
    pub probes: Vec<ProbeSpec>,
    pub manual_reconciliation: Vec<ManualStateRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct IacOperation {
    pub project: ProjectName,
    pub workspace: WorkspaceId,
    pub provider: IacProvider,
    pub environment: EnvironmentName,
    pub stack: String,
    pub preview_command: ProcessCommand,
    pub apply_command: Option<ProcessCommand>,
    pub risk: Vec<RiskFlag>,
}

pub struct DnsOperation {
    pub zone: String,
    pub provider: DnsProvider,
    pub record: String,
    pub expected_target: String,
    pub expected_proxied: Option<bool>,
    pub verification: Vec<ProcessCommand>,
}

pub struct ProbeSpec {
    pub name: String,
    pub method: HttpMethod,
    pub url: String,
    pub expected: ProbeExpectation,
    pub classification: Option<ProbeFailureClass>,
}

pub struct SessionJournal {
    pub id: SessionId,
    pub plan_id: Option<OpsPlanId>,
    pub entries: Vec<SessionEntry>,
    pub redaction: RedactionPolicy,
}
```

## 13. Acceptance Criteria

Create a fixture equivalent to the reviewed `universe` change:

```text
repo/
  core-infra/nucleus/
  frameworks/operon/operon-infra/
  apps/golgi/apps/slides/infra/
  foundations/ligand/infra/
  foundations/atp/infra/
  foundations/chromatin/infra/
  apps/histone/apps/histone-server/infra/
  apps/histone/apps/histone-web/infra/
  apps/versicle/infra/
```

The fixture change should include:

- Cloudflare dependency added to DNS-owning IaC;
- Cloudflare DNS-only records replacing Route53 app records;
- shared Operon Lambda Function URL auth change;
- a provider capability gap for `invokedViaFunctionUrl`;
- consumer `package.json` and `package-lock.json` changes;
- declared probes for CloudFront and app-level callback behavior.

Run:

```bash
repoctl graph validate
repoctl affected --base HEAD~1 --head HEAD --tasks check,test,build --format json
repoctl run check --affected --base HEAD~1 --head HEAD --dry-run
repoctl ops plan --base HEAD~1 --head HEAD --env staging --format json
repoctl ops verify --plan target/repoctl/ops-plan.json
repoctl pr summary --base HEAD~1 --head HEAD
```

Expected:

- affected tasks are unique;
- `repoctl run check --affected --dry-run` plans commands or emits precise
  missing-command diagnostics;
- `ops plan` lists nucleus before app/foundation stack verification where
  certificate/DNS dependencies require it;
- `ops plan` lists `ligand`, `atp`, and `slides` runtime dependency probes;
- Cloudflare records are checked for `proxied=false`;
- CloudFront checks assert response headers come from CloudFront;
- provider capability report identifies the Pulumi AWS 6 field gap without
  recommending a blind provider major upgrade;
- manual Lambda permission additions can be recorded and later reconciled;
- PR summary includes DNS/CDN/runtime verification commands and production gaps;
- no secret values appear in JSON, Markdown, or command logs.

## 14. Implementation Phases

Phase 1: affected/run correctness

- Deduplicate affected tasks.
- Make `run --affected` consume the same base/head and changed-file model as
  `affected`.
- Add stable diagnostics when no commands are planned.

Phase 2: operations plan skeleton

- Add `OpsPlan` and `repoctl ops plan`.
- Include affected report, task report, IaC stack commands, and risk flags.
- Add human and JSON renderers.

Phase 3: DNS/CDN/probe model

- Add manifest schema for DNS intent, CDN intent, and probes.
- Implement DNS-only Cloudflare verification adapter.
- Implement generic HTTP probe runner with failure classification.

Phase 4: provider capability diagnostics

- Add Pulumi provider package/schema inspection for TypeScript workspaces.
- Detect provider major upgrades and unsupported fields.
- Surface capability diagnostics in ops plans and PR summaries.

Phase 5: session journal and reconciliation

- Add local session journal files under `target/repoctl/sessions`.
- Add redaction rules.
- Add manual-state reconciliation records and reports.

Phase 6: end-to-end operational fixture

- Build the `universe`-like fixture.
- Verify graph, affected, run dry-run, ops plan, ops verify, and PR summary
  behavior end to end.
