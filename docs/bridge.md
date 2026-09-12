# Wine bridge setup

`Navmut-windows.zip` contains `navmut-bridge.exe`,
`navmut-helper.exe`, and `navmut-helper-hook.dll`. All three are Windows x86
binaries. Use these files with the native Linux/macOS Navmut app
to test live game access through the game's Wine environment.

## Start the bridge

1. Download `Navmut-windows.zip` and extract the entire `Navmut` folder.
2. Start the game in its usual Wine prefix.
3. Run `navmut-bridge.exe` in that same prefix and pass a new connection file
   path. Keep both helper files beside the bridge executable.

For a standard Wine installation, from the extracted folder:

```sh
export WINEPREFIX="/absolute/path/to/game-prefix"
wine ./navmut-bridge.exe 'C:\navmut-connection.json'
```

For a managed Wine prefix, use its executable runner and pass
`C:\navmut-connection.json` as the argument. The runner must support the
Windows x86 game and bridge binaries.

Leave the bridge running. It writes the selected local port and an
authentication token to the connection file. Do not share that file.

## Connect Navmut

Start native Navmut with `--bridge` and the host filesystem path to the same
connection file. For example, on Linux:

```sh
./Navmut_1.0.0_amd64.AppImage --bridge "$WINEPREFIX/drive_c/navmut-connection.json"
```

On macOS, launch the executable inside the installed app bundle, for example:

```sh
/Applications/Navmut.app/Contents/MacOS/navmut --bridge "/absolute/path/to/game-prefix/drive_c/navmut-connection.json"
```

The bridge and native app communicate over `127.0.0.1` on the same computer.
No server IP is needed. The bridge accesses the game client, not the game server.

Stop the bridge with Ctrl+C when finished. If its connection file remains,
delete it after the bridge has stopped before starting another session with
that path; the bridge refuses to overwrite an existing connection file.
