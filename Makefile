PUBLISH_CRATES := repoctl-core repoctl-engine repoctl-runner repoctl-scaffold repoctl-inspect repoctl repoctl-cli

build:
	@cargo build

test:
	@cargo nextest run --all-features

check-agent-sync:
	@cmp -s CLAUDE.md AGENTS.md || { \
		echo "AGENTS.md must stay in sync with CLAUDE.md"; \
		echo "Update both files with the same shared project instructions."; \
		exit 1; \
	}
	@tmp_dir=$$(mktemp -d); \
	trap 'rm -rf "$$tmp_dir"' EXIT; \
	cp -R .claude/skills "$$tmp_dir/expected-skills"; \
	find "$$tmp_dir/expected-skills" -name SKILL.md -exec perl -0pi -e 's/CLAUDE\.md/AGENTS.md/g; s/Claude/Codex/g; s/claude/codex/g' {} +; \
	diff -ru --exclude agents "$$tmp_dir/expected-skills" .agents/skills || { \
		echo "Codex skills must stay in sync with Claude skills after Claude-to-Codex renaming."; \
		echo "Update .claude/skills first, then mirror the shared content into .agents/skills."; \
		exit 1; \
	}

release:
	@cargo release tag --execute
	@git cliff -o CHANGELOG.md
	@git commit -a -n -m "Update CHANGELOG.md" || true
	@git push origin master
	@cargo release push --execute

publish-dry-run:
	@set -e; \
	for crate in $(PUBLISH_CRATES); do \
		cargo publish -p "$$crate" --dry-run --allow-dirty; \
	done

publish:
	@set -e; \
	for crate in $(PUBLISH_CRATES); do \
		cargo publish -p "$$crate"; \
	done

update-submodule:
	@git submodule update --init --recursive --remote

.PHONY: build test check-agent-sync release publish-dry-run publish update-submodule
