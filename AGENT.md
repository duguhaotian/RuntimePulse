# RuntimePulse Agent Guidelines

## Build, validation, and test execution

All project build, validation, and test commands must be executed inside containers. Do not run project builds, validators, or test suites directly on the host environment.

Preferred patterns:

- Use `docker compose build ...` for image builds.
- Use `docker compose run --rm <service> ...` or `docker compose exec <service> ...` for service-specific validation and tests.
- If a one-off tool is required, run it through an appropriate container image with the repository mounted, rather than installing/running it on the host.

Host-side commands should be limited to lightweight repository inspection/editing and container orchestration, such as `git`, `rg`, `sed`, `docker compose ps`, `docker compose restart`, and `docker compose logs`.

## Code modification workflow

When modifying code:

- Keep changes scoped to the requested task and avoid unrelated refactors.
- Preserve existing user or teammate edits; do not overwrite or revert unrelated work.
- After changing code that affects a containerized service, rebuild/recreate the affected container image before reporting that the running UI/service has the change.
- Record the container-based build, validation, or test command that was run when summarizing the change.
- Archive completed work with a git commit when asked to 归档代码.
