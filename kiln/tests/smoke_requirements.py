"""Hermetic test of the Nexus requirements check: real gaps reported, noise filtered
(optional, VR, edition-specific, INI-only add-ons, modern replacements, non-framework
patches), fixed with one click, and cached.
Usage: python3 tests/smoke_requirements.py <empty work dir>"""
import os, struct, sys, threading
from pathlib import Path

work = Path(sys.argv[1]).resolve(); work.mkdir(parents=True, exist_ok=True)
assert not any(work.iterdir()), "work dir must be empty"
os.environ.update(KILN_NO_WELCOME="1", HOME=str(work / "home"), XDG_CONFIG_HOME=str(work / "home/.config"),
                  XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import steam, nexus, requirements, doctor
lib = work / "lib"
steam.libraries = lambda: [lib]; steam.steam_roots = lambda: [work / "steamroot"]
sk = lib / "steamapps/common/Skyrim Special Edition"; (sk / "Data").mkdir(parents=True)
ms, ls = (1 << 16) | 6, (1170 << 16)
(sk / "SkyrimSE.exe").write_bytes(b"MZ" + b"\0" * 62 + struct.pack("<IIII", 0xFEEF04BD, 0x10000, ms, ls))

from kiln.games import by_id
from kiln.instance import Instance, Mod
inst = Instance(by_id("skyrimse"), sk, work / "staging"); inst.load()


def add(mid, nid, files, enabled=True, name=None):
    m = Mod(id=mid, name=name or mid, enabled=enabled, nexus_mod_id=nid, nexus_file_id=1)
    for rel in files:
        f = inst.mod_path(m) / rel; f.parent.mkdir(parents=True, exist_ok=True); f.write_text("x")
    inst.mods.append(m)


add("FormList Manipulator", 74037, ["SKSE/Plugins/FormListManipulator.dll"])
add("po3 Tweaks INI", 51073, ["SKSE/Plugins/po3_Tweaks.ini"], name="powerofthree's Tweaks: INI file")
add("powerofthree's Tweaks", 51073, ["SKSE/Plugins/po3_Tweaks.dll"], enabled=False)
add("Pandora", 133232, ["Pandora Behaviour Engine+.exe"], name="Pandora Behaviour Engine Plus")
add("Some Patch", 900, ["SomePatch.esp"])
add("Ini Mod", 901, ["SKSE/Plugins/x.dll"])
inst.save()

R = lambda mid, name, notes="", ext=False, game="1704": {"modId": str(mid), "modName": name, "notes": notes,
                                                         "externalRequirement": ext, "gameId": game}
REQS = {
    74037: [R(51073, "powerofthree's Tweaks", "Mandatory - Load Editor IDs must be enabled"),
            R(59510, "powerofthree's Tweaks VR"),                         # VR: skipped
            R(74689, "MergeMapper", "[Optional] for VR players"),         # optional: skipped
            R(32444, "Address Library for SKSE Plugins", "Mandatory")],   # not downloaded: install
    901: [R(60033, "Nemesis Unlimited Behavior Engine"),                   # Pandora does the job
          R(78146, "Behavior Data Injector", "SE"),                       # 1.6.1170 is AE: skipped
          R(5555, "Some Website Tool", ext=True)],                        # external: skipped
    900: [R(777, "Big City Mod")],                                         # a content mod: missing masters' job
}
CATS = {32444: "Modders Resources", 777: "Locations - New", 51073: "Utilities"}
calls = []


def fake_gql(q, v):
    calls.append(len(v["ids"]))
    nodes = []
    for i in v["ids"]:
        mid = i["modId"]
        nodes.append({"modId": str(mid), "modCategory": {"name": CATS.get(mid, "Gameplay")},
                      "modRequirements": {"nexusRequirements": {"nodes": REQS.get(mid, [])}}})
    return {"legacyModsByDomain": {"nodes": nodes}}
nexus._gql = fake_gql

found = requirements.check(inst)
print("missing:", [(e.name, e.have.id if e.have else None, [m.id for m in e.needed_by]) for e in found])
assert [e.nexus_id for e in found] == [51073, 32444] or [e.nexus_id for e in found] == [32444, 51073], found
po3 = next(e for e in found if e.nexus_id == 51073)
assert po3.have.id == "powerofthree's Tweaks", "the INI-only add-on doesn't count, the real plugin is offered"
before = len(calls)
requirements.check(inst)
assert len(calls) == before, "second check comes from the cache"
print("cached ✔ | filtered: VR, optional, SE-only, external, Pandora for Nemesis, content-mod patch")

# Troubleshoot (in a worker thread it may go online; the window thread uses the cache)
res = {}
t = threading.Thread(target=lambda: res.setdefault("i", doctor.diagnose(inst))); t.start(); t.join()
iss = {i.key: i for i in res["i"] if i.source == "requirements"}
print("Troubleshoot:", [(i.title, i.actions[0].fix) for i in iss.values()])
assert iss["req:51073"].actions[0].fix == "add_to_profile:powerofthree's Tweaks" and iss["req:51073"].actions[0].auto
assert iss["req:32444"].actions[0].fix == "install_nexus:32444"
plan = [a.fix for _i, a in doctor.plan_fix_all(res["i"])]
assert "add_to_profile:powerofthree's Tweaks" in plan
doctor.apply(inst, "add_to_profile:powerofthree's Tweaks")
assert inst.get("powerofthree's Tweaks").enabled and inst.in_profile(inst.get("powerofthree's Tweaks"))
left = [e.nexus_id for e in requirements.check(inst)]
print("after Fix:", left)
assert left == [32444]
print("requirements smoke OK")
