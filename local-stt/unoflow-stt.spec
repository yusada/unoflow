# PyInstaller spec for the standalone UnoFlow local STT server.
#
# Produces `dist/unoflow-stt/unoflow-stt.exe` (a one-folder build): the exe plus
# the native libraries faster-whisper needs. One-folder is used deliberately —
# a one-file build would unpack ~200 MB to a temp dir on every launch.
#
# Build (from this directory):
#   py -3.10 -m PyInstaller --noconfirm unoflow-stt.spec
#
# The Whisper model is NOT baked in here: it ships alongside as `models/tiny/`
# so the app and the model can be updated independently.

from PyInstaller.utils.hooks import collect_dynamic_libs, collect_data_files

binaries = []
datas = []

# ctranslate2 and onnxruntime load native DLLs at runtime; PyInstaller does not
# always pick these up automatically.
for pkg in ("ctranslate2", "onnxruntime"):
    binaries += collect_dynamic_libs(pkg)
    datas += collect_data_files(pkg)

# faster-whisper ships its VAD assets as package data.
datas += collect_data_files("faster_whisper")

a = Analysis(
    ["server.py"],
    pathex=[],
    binaries=binaries,
    datas=datas,
    hiddenimports=[
        "uvicorn.logging",
        "uvicorn.loops.auto",
        "uvicorn.protocols.http.auto",
        "uvicorn.protocols.websockets.auto",
        "uvicorn.lifespan.on",
    ],
    hookspath=[],
    runtime_hooks=[],
    excludes=[
        "tkinter",
        "matplotlib",
        "pytest",
        "setuptools",
        # Not used by the server; PyInstaller picks them up via transitive
        # imports (PIL ~11 MB, pywin32 ~7 MB).
        "PIL",
        "win32api",
        "win32con",
        "win32gui",
        "win32ui",
        "pythonwin",
    ],
    noarchive=False,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="unoflow-stt",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=False,  # no console window: the app launches it in the background
)

coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=False,
    name="unoflow-stt",
)
