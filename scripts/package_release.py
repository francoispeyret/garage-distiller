#!/usr/bin/env python3
"""Package the native executable. Signing the update ZIP is a separate CI step."""
import argparse
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import zipfile


def package(target: str, version: str, binary: Path, output: Path) -> Path:
    if target not in {"aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc"}:
        raise ValueError(f"Unsupported release target: {target}")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"garage-distiller-v{version}-{target}.zip"
    with tempfile.TemporaryDirectory(prefix="garage-package-") as temp:
        staging = Path(temp)
        if target.endswith("apple-darwin"):
            app = staging / "Garage Distiller.app"
            executable = app / "Contents" / "MacOS" / "garage-distiller"
            executable.parent.mkdir(parents=True)
            shutil.copy2(binary, executable)
            executable.chmod(0o755)
            info = {
                "CFBundleName": "Garage Distiller",
                "CFBundleDisplayName": "Garage Distiller",
                "CFBundleExecutable": "garage-distiller",
                "CFBundleIdentifier": "fr.francoispeyret.garage-distiller",
                "CFBundlePackageType": "APPL",
                "CFBundleShortVersionString": version,
                "CFBundleVersion": version,
                "LSMinimumSystemVersion": "12.0",
                "NSHighResolutionCapable": True,
                "NSPrincipalClass": "NSApplication",
            }
            icon = Path(__file__).resolve().parents[1] / "assets" / "icon.icns"
            if icon.is_file():
                resources = app / "Contents" / "Resources"
                resources.mkdir(parents=True)
                shutil.copy2(icon, resources / "icon.icns")
                info["CFBundleIconFile"] = "icon.icns"
            with (app / "Contents" / "Info.plist").open("wb") as stream:
                plistlib.dump(info, stream)
            # Ad-hoc signing supports local execution; this is NOT Apple notarization.
            subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
            subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        else:
            shutil.copy2(binary, staging / "garage-distiller.exe")
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            for path in sorted(staging.rglob("*")):
                if path.is_file():
                    z.write(path, path.relative_to(staging).as_posix())
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    print(package(args.target, args.version, args.binary, args.output))
