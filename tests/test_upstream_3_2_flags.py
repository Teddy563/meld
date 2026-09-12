"""The arnis 3.2.0 options, and the promise that a 3.1.8 binary never sees one.

Meld and the generator ship separately, so a 1.9.9 Meld sitting next to the 3.1.8 fork is
the normal case, not the edge case. clap rejects an unknown argument outright - one
ungated flag turns every cell into `error: unexpected argument '--voxy-lod'` - so every
new option is gated on arnis_supports(), which greps the binary's own --help.

The two claims worth a test:

  1. Against a binary that advertises nothing new, build_arnis_cmd emits nothing new. That
     is the byte-identical-to-1.9.8 guarantee; it is what makes the version bump safe to
     ship before the arnis merge lands.
  2. Against a binary that does advertise them, each option appears - and only when the
     setting differs from the generator's own default, so a project nobody touched still
     renders the same world.

The Mapillary token is checked for where it must NOT be: on the command line. It travels
in the child environment instead, because argv is readable by other processes.
"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from src import arnis_cmd  # noqa: E402

BBOX = {"south": 44.0, "west": 26.0, "north": 44.1, "east": 26.1}
ORIGIN = {"lat": 44.0, "lng": 26.0}

ALL_NEW = (
    "--voxy-lod", "--building-facades", "--body", "--overture-source",
    "--facade-detail", "--facade-px", "--mapillary-facade-mode", "--mapillary-facades",
)


@pytest.fixture(autouse=True)
def _clear_help_cache():
    arnis_cmd._HELP_CACHE.clear()
    yield
    arnis_cmd._HELP_CACHE.clear()


def _cmd(monkeypatch, settings, *, supports=()):
    monkeypatch.setattr(arnis_cmd, "arnis_supports", lambda exe, flag: flag in supports)
    # These tests are about WHICH flag a setting produces, so they run in enhanced mode;
    # the classic gate has its own tests at the bottom and sets the key itself.
    settings = {"gen_mode_32": "enhanced", **settings}
    return arnis_cmd.build_arnis_cmd("arnis.exe", BBOX, "out", settings, ORIGIN, None, 1)


# --- claim 1: an old binary sees nothing new --------------------------------------------


@pytest.mark.parametrize("flag", ALL_NEW)
def test_no_new_flag_reaches_a_binary_that_does_not_advertise_it(monkeypatch, flag):
    """Every option set to a non-default value, and a generator that advertises none."""
    settings = {
        "voxy_lod": True, "building_facades": True, "body": "mars",
        "overture_source": "tiles", "facade_detail": "high", "facade_px": "32",
        "mapillary_facade_mode": "blocks", "mapillary_facades": False,
    }
    assert flag not in _cmd(monkeypatch, settings, supports=())


def test_the_command_line_is_unchanged_when_the_new_settings_are_absent(monkeypatch):
    """A project stored by 1.9.8 has none of these keys, and gets the 1.9.8 command line
    even from a generator that does advertise every one of them."""
    before = _cmd(monkeypatch, {}, supports=())
    after = _cmd(monkeypatch, {}, supports=ALL_NEW)
    assert before == after


# --- claim 2: a 3.2.0 binary gets what was asked for -------------------------------------


def test_bare_flags_are_emitted_when_supported_and_on(monkeypatch):
    cmd = _cmd(monkeypatch, {"voxy_lod": True, "building_facades": True}, supports=ALL_NEW)
    assert "--voxy-lod" in cmd
    assert "--building-facades" in cmd


def test_bare_flags_stay_off_when_the_setting_is_off(monkeypatch):
    cmd = _cmd(monkeypatch, {"voxy_lod": False, "building_facades": False}, supports=ALL_NEW)
    assert "--voxy-lod" not in cmd
    assert "--building-facades" not in cmd


@pytest.mark.parametrize("key,flag,value", [
    ("body", "--body", "moon"),
    ("body", "--body", "mars"),
    ("overture_source", "--overture-source", "tiles"),
    ("overture_source", "--overture-source", "parquet"),
    ("facade_detail", "--facade-detail", "high"),
    ("facade_px", "--facade-px", "4"),
    ("facade_px", "--facade-px", "32"),
    ("mapillary_facade_mode", "--mapillary-facade-mode", "blocks"),
])
def test_value_flags_carry_their_value(monkeypatch, key, flag, value):
    cmd = _cmd(monkeypatch, {key: value}, supports=ALL_NEW)
    assert cmd[cmd.index(flag) + 1] == value


@pytest.mark.parametrize("key,flag,default", [
    ("body", "--body", "earth"),
    ("overture_source", "--overture-source", "auto"),
    ("facade_detail", "--facade-detail", "standard"),
    ("facade_px", "--facade-px", "16"),
    ("mapillary_facade_mode", "--mapillary-facade-mode", "photos"),
])
def test_the_generators_own_default_is_left_unsaid(monkeypatch, key, flag, default):
    """Saying the default out loud would be harmless today and a silent pin tomorrow: the
    next generator that moves a default would keep rendering the old one for every project
    Meld ever wrote."""
    assert flag not in _cmd(monkeypatch, {key: default}, supports=ALL_NEW)


@pytest.mark.parametrize("value", ["", None, "sun", "jupiter", "  "])
def test_a_value_the_generator_would_reject_is_dropped_not_forwarded(monkeypatch, value):
    """clap would fail the whole cell on an unknown enum value. A settings file can carry
    one - hand-edited, or written by a newer Meld - so it is filtered here rather than
    turned into a usage error thousands of times over a country render."""
    assert "--body" not in _cmd(monkeypatch, {"body": value}, supports=ALL_NEW)


def test_values_are_case_and_space_insensitive(monkeypatch):
    cmd = _cmd(monkeypatch, {"body": "  Mars "}, supports=ALL_NEW)
    assert cmd[cmd.index("--body") + 1] == "mars"


# --- the Mapillary tri-state --------------------------------------------------------------


def test_facades_off_is_stated_explicitly(monkeypatch):
    """Upstream turns facades on as soon as a token exists, so 'off' is the only half of
    this setting that has anything to say."""
    cmd = _cmd(monkeypatch, {"mapillary_facades": False}, supports=ALL_NEW)
    assert cmd[cmd.index("--mapillary-facades") + 1] == "false"


@pytest.mark.parametrize("value", [None, True])
def test_follow_the_token_says_nothing(monkeypatch, value):
    assert "--mapillary-facades" not in _cmd(
        monkeypatch, {"mapillary_facades": value}, supports=ALL_NEW)


# --- the credential -----------------------------------------------------------------------


def test_the_token_never_appears_on_the_command_line(monkeypatch):
    """argv is readable by any other process on the machine. The token goes to the child's
    environment as MAPILLARY_TOKEN (server.py), never here."""
    secret = "MLY|1234567890|deadbeefcafe"
    cmd = _cmd(monkeypatch, {"mapillary_token": secret}, supports=ALL_NEW + ("--mapillary-token",))
    assert secret not in cmd
    assert "--mapillary-token" not in cmd


def test_the_token_is_stripped_from_a_shared_preset():
    """A preset is made to be handed to someone else. The recipient brings their own token."""
    from src import presets  # noqa: PLC0415

    kept, _, _ = presets.clean_settings(
        {"mapillary_token": "MLY|secret", "body": "mars"}, known_only=False)
    assert "mapillary_token" not in kept
    assert kept["body"] == "mars"


def test_the_token_is_stripped_from_a_worlds_metadata_sidecar():
    """meld-world.json is written into the world folder, and world folders get zipped up
    and passed around."""
    import server  # noqa: PLC0415

    assert "mapillary_token" in server._META_SKIP_SETTINGS


# --- the classic / enhanced switch ---------------------------------------------------------


# The switch gates the features that change an ordinary Earth render without being asked -
# the facades and the Overture transport. What the user picks by name (Voxy, the body) is
# not gated: those emit nothing at all when left alone.
CLASSIC_FREE = ("--voxy-lod", "--body")
CLASSIC_GATED = tuple(f for f in ALL_NEW if f not in CLASSIC_FREE)


@pytest.mark.parametrize("flag", CLASSIC_GATED)
def test_classic_mode_emits_no_world_changing_flag_even_when_supported(monkeypatch, flag):
    """The switch is what lets a project go back to the command line it rendered with: in
    classic none of the flags that change the world are sent, whatever the settings say."""
    settings = {
        "gen_mode_32": "classic",
        "voxy_lod": True, "building_facades": True, "body": "mars",
        "overture_source": "tiles", "facade_detail": "high", "facade_px": "32",
        "mapillary_facade_mode": "blocks", "mapillary_facades": False,
    }
    assert flag not in _cmd(monkeypatch, settings, supports=ALL_NEW)


def test_classic_still_allows_voxy(monkeypatch):
    """It does not change the world, so hiding it behind Enhanced only hid a render option
    behind a look-of-the-world switch."""
    settings = {"gen_mode_32": "classic", "voxy_lod": True}
    assert "--voxy-lod" in _cmd(monkeypatch, settings, supports=ALL_NEW)


def test_classic_still_allows_the_body(monkeypatch):
    """Moon and Mars are not a style applied to a render, they are the render."""
    settings = {"gen_mode_32": "classic", "body": "mars"}
    cmd = _cmd(monkeypatch, settings, supports=ALL_NEW)
    assert cmd[cmd.index("--body") + 1] == "mars"


def test_classic_left_alone_emits_nothing(monkeypatch):
    """The Classic guarantee: untouched, the command line is 1.9.8's, byte for byte - the
    two ungated options emit nothing at their defaults."""
    settings = {"gen_mode_32": "classic", "voxy_lod": False, "body": "earth"}
    cmd = _cmd(monkeypatch, settings, supports=ALL_NEW)
    assert not [a for a in cmd if a in ALL_NEW]


def test_classic_is_the_default_when_the_key_is_absent(monkeypatch):
    """A project stored before the switch existed has no gen_mode_32 and must stay classic:
    the gated features stay off, whatever its settings happen to hold."""
    monkeypatch.setattr(arnis_cmd, "arnis_supports", lambda exe, flag: flag in ALL_NEW)
    settings = {"building_facades": True, "overture_source": "tiles", "facade_detail": "high"}
    cmd = arnis_cmd.build_arnis_cmd("arnis.exe", BBOX, "out", settings, ORIGIN, None, 1)
    assert not [a for a in cmd if a in CLASSIC_GATED]


def test_enhanced_mode_lets_them_through(monkeypatch):
    cmd = _cmd(monkeypatch, {"gen_mode_32": "enhanced", "voxy_lod": True}, supports=ALL_NEW)
    assert "--voxy-lod" in cmd
