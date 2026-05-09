# Git Workflow

This project should use a simple feature-branch workflow.

## Check Current Changes

```bash
git status
```

Review changed files:

```bash
git diff --stat
git diff
```

## Build Before Commit

Use the container workflow:

```bash
docker compose build
```

Optionally run the service:

```bash
docker compose up -d
curl -I http://localhost:8080
```

## Create a Branch

If you are on `main`, create a feature branch:

```bash
git checkout -b feature/frontend-prototype
```

If the branch already exists:

```bash
git checkout feature/frontend-prototype
```

## Stage Files

Stage the frontend and docs:

```bash
git add README.md docker-compose.yml frontend docs
```

Check staged files:

```bash
git status
```

## Commit

Example commit message:

```bash
git commit -m "Add frontend-first RuntimePulse prototype"
```

Alternative if focusing on current UI work:

```bash
git commit -m "Add W&B-style sandbox runs UI prototype"
```

## Push

If this is a new branch:

```bash
git push -u origin feature/frontend-prototype
```

If the upstream already exists:

```bash
git push
```

## Open a Pull Request

After pushing, open a PR from:

```text
feature/frontend-prototype -> main
```

Suggested PR summary:

```text
- Adds containerized React frontend prototype.
- Adds mock RuntimePulse API and scenario telemetry.
- Adds W&B-style Runs and Reports UI for sandbox analysis.
- Adds architecture and development plan docs.
```

## Notes

Do not commit generated local dependencies such as `node_modules` or frontend build output. The current `.dockerignore` excludes them from Docker context, but Git should also avoid tracking them.
