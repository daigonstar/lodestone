import argparse
import sys


def main() -> int:
    ap = argparse.ArgumentParser(prog="kiln", description="Mod manager for Linux with Nexus Mods integration")
    ap.add_argument("uri", nargs="?", help="an nxm:// link (from the Nexus 'Mod Manager Download' button)")
    ap.add_argument("--register-nxm", action="store_true", help="make Kiln the nxm:// handler and exit")
    ap.add_argument("--install-desktop", action="store_true",
                    help="add Kiln to the app launcher without taking over nxm:// links, then exit")
    ap.add_argument("--games", action="store_true", help="print supported games as JSON and exit")
    ap.add_argument("--status", metavar="GAME", help="print a game's mod status as JSON and exit")
    ap.add_argument("--game", metavar="GAME", help="open Kiln on this game")
    ap.add_argument("--play", metavar="GAME", help="Guarded Play: check the game's mods, then launch it")
    ap.add_argument("--play-vanilla", metavar="GAME", help="play without mods (they're put back on the next Play)")
    args = ap.parse_args()

    if args.games or args.status:
        from . import cli
        return cli.print_json(cli.games() if args.games else cli.status(args.status))
    if args.game or args.play or args.play_vanilla:
        from . import cli
        if args.play_vanilla:
            args.uri = cli.VANILLA_CMD + args.play_vanilla
        else:
            args.uri = (cli.PLAY_CMD + args.play) if args.play else (cli.GAME_CMD + args.game)

    if args.register_nxm or args.install_desktop:
        from . import desktop
        p = desktop.register_nxm() if args.register_nxm else desktop.write_desktop_entry(False)
        print("Wrote", p)
        return 0

    from .gui import run
    return run(args.uri)


if __name__ == "__main__":
    sys.exit(main())
