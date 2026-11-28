GENERATED := 'crates/*/src/generated/*' 'apps/*/src/generated/*'

.PHONY: check generate format

node_modules/.modules.yaml: package.json pnpm-lock.yaml pnpm-workspace.yaml apps/*/package.json
	vp install --frozen-lockfile

check: node_modules/.modules.yaml
	nix fmt -- --ci
	lake -d lean build --wfail
	lake -d audit build --wfail
	leanfran generate
	git diff --exit-code -- $(GENERATED)
	test -z "$$(git ls-files --others --exclude-standard -- $(GENERATED))"
	cargo clippy --all-targets -- -D warnings
	cargo test
	vp run check

generate:
	leanfran generate

format:
	nix fmt
	vp fmt
