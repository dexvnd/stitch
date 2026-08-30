<div align="center">

  # Stitch

  **Inject and run live Python code inside any running process**

  ![Rust](https://img.shields.io/badge/Rust-2024-CE422B?style=flat&logo=rust&logoColor=white)
  ![Platform](https://img.shields.io/badge/Platform-Windows-0078D6?style=flat&logo=windows&logoColor=white)
  ![egui](https://img.shields.io/badge/UI-egui-lightgrey?style=flat)

  <img src="https://skidding.dev/content/cdn/HipEqyMEfYiN.gif" alt="Stitch in action" width="680"/>

</div>

---

Inspired by [Pyinjector](https://github.com/call-042PE/PyInjector), Stitch lets you inject Python code to any running Python process and execute code inside it in real time. Pick a process, connect, and start injecting Python code.

## What it does

Stitch finds Python processes running on your machine, attaches to one, and opens a live channel into it. From there you can execute any Python code and see the output immediately.

## Features

- Automatically finds running Python processes and shows which version they're using
- Live code execution.
- Script folder with support for custom `.py` files.
- Create new scripts directly from the UI
- No dependencies required on the target machine, Stitch uses the Python interpreter already running in the process

## Installation

Grab the latest `stitch.exe` from the [Releases](https://github.com/dexvnd/stitch/releases) page, or build it yourself from source.

### Build from source

You'll need Rust installed.

```bash
git clone https://github.com/dexvnd/stitch.git
cd stitch
cargo build --release
```

The build script compiles the stub DLL and embeds it into `stitch.exe` automatically.

## Usage

1. Start a Python process (any script, app, or just `python` in a terminal)
2. Launch `stitch.exe`
3. Select the process from the list on the left and click **Connect**
4. Type or load a script and click **Execute**

## How it works

When you connect to a python process Stitch injects a small stub into the target process using the standard Windows loader API. The stub initialises the CPython runtime inside the target, connects back to Stitch over a named pipe. Reading code, executing it under the GIL, and sending the output back. The pipe stays open so you can keep sending code without reconnecting.

The whole thing is written in Rust. The stub is a `cdylib` target in the same workspace, compiled separately and the injection and UI are in the main binary.

## Project structure

```
stitch/
├── src/
│   ├── main.rs        entry point
│   ├── app.rs         egui UI
│   ├── lib.rs         stub DLL
│   ├── python.rs      CPython API resolved at runtime
│   ├── pipe.rs        named pipe for communicating between exe and stub
│   ├── injector.rs    loads the stub into the target process
│   └── process.rs     enumerates and filters Python processes
```

## Credits

Project idea and name by [Body-Alhoha](https://github.com/Body-Alhoha).

## License

MIT — see [LICENSE](LICENSE) for details.

---

<div align="center">
  <sub>Built with Rust, egui, and windows-rs</sub>
</div>