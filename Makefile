.PHONY: check probe audit
check:
	cargo fmt --check
	cargo clippy --all-targets --all-features -- -D warnings
	cargo test
	cargo build --locked
	python3 tests/terminal_smoke.py
	python3 tests/board_smoke.py
	python3 tests/board_errors.py
	python3 tests/board_handoff.py
	python3 tests/board_ack.py
	python3 tests/board_picker.py
	python3 tests/board_search.py
	python3 tests/board_naming.py
	python3 tests/naming_upgrade.py
	python3 tests/workspace.py
	sh -n install.sh
	python3 tests/installer.py
	python3 tests/release_package.py
	python3 tests/installed_binary_test.py
	python3 tests/beta_publish.py

probe:
	cargo test --features probe -- --ignored

audit:
	cargo audit --deny warnings
