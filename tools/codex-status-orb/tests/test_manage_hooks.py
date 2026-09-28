from __future__ import annotations

from pathlib import Path

from manage_hooks import MARKER, discover_codex_homes, install_home, install_homes, load_hooks


def test_discovers_default_environment_and_named_homes(tmp_path: Path):
    default = tmp_path / ".codex"
    active = tmp_path / ".codex-cy"
    other = tmp_path / ".codex-feu"
    ignored = tmp_path / ".codex-temp"
    for home in (active, other):
        home.mkdir()
        (home / "config.toml").write_text("", encoding="utf-8")
    ignored.mkdir()

    homes = discover_codex_homes(tmp_path, str(active))

    assert homes == [default.resolve(), active.resolve(), other.resolve()]


def test_install_home_preserves_unrelated_hooks(tmp_path: Path):
    codex_home = tmp_path / ".codex-test"
    codex_home.mkdir()
    hooks_path = codex_home / "hooks.json"
    hooks_path.write_text(
        '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"keep-me","statusMessage":"Other"}]}]}}',
        encoding="utf-8",
    )

    install_home(codex_home)
    data = load_hooks(hooks_path)
    stop_groups = data["hooks"]["Stop"]

    assert any(group["hooks"][0].get("command") == "keep-me" for group in stop_groups)
    assert sum(group["hooks"][0].get("statusMessage") == MARKER for group in stop_groups) == 1
    assert (codex_home / "backups").is_dir()


def test_windows_command_does_not_quote_executable(tmp_path: Path):
    codex_home = tmp_path / ".codex-test"
    install_home(codex_home)
    data = load_hooks(codex_home / "hooks.json")
    handler = data["hooks"]["SessionStart"][0]["hooks"][0]

    assert not handler["commandWindows"].startswith('"')
    assert handler["commandWindows"].endswith('hook.py"')
    assert data["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"] == 10
    assert data["hooks"]["UserPromptSubmit"][0]["hooks"][0]["timeout"] == 15


def test_install_homes_adds_status_orb_to_every_profile(tmp_path: Path):
    default = tmp_path / ".codex"
    named = tmp_path / ".codex-cy"
    for home in (default, named):
        home.mkdir()
        (home / "hooks.json").write_text(
            '{"hooks":{"Stop":['
            '{"hooks":[{"type":"command","command":"orb","statusMessage":"Codex Status Orb"}]},'
            '{"hooks":[{"type":"command","command":"keep-me","statusMessage":"Central Observability"}]}'
            ']}}',
            encoding="utf-8",
        )

    install_homes([default.resolve(), named.resolve()])

    default_stop = load_hooks(default / "hooks.json")["hooks"]["Stop"]
    named_stop = load_hooks(named / "hooks.json")["hooks"]["Stop"]
    assert sum(group["hooks"][0].get("statusMessage") == MARKER for group in default_stop) == 1
    assert sum(group["hooks"][0].get("statusMessage") == MARKER for group in named_stop) == 1
    assert any(group["hooks"][0].get("command") == "keep-me" for group in named_stop)
