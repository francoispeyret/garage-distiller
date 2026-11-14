#!/usr/bin/env python3
"""Set up the repository's update-signing key exactly once, without logging secrets.

Requires gh authenticated with repository administration access and zipsign 0.2.2.
Existing remote keys are never overwritten. Private material only lives in a
temporary directory and the GitHub Actions secret; this script keeps no backup.
"""
import argparse
import base64
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

REPOSITORY = "francoispeyret/garage-distiller"
SECRET = "GARAGE_DISTILLER_UPDATE_PRIVATE_KEY_BASE64"
VARIABLE = "GARAGE_DISTILLER_UPDATE_PUBLIC_KEY_HEX"
PUBLIC_FILE = Path(__file__).resolve().parents[1] / "assets" / "update-public-key.txt"


def github(*args: str, stdin: str | None = None) -> str:
    result = subprocess.run(
        ["gh", *args, "--repo", REPOSITORY],
        input=stdin,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        # Never echo command input or subprocess diagnostics from a secret write.
        raise RuntimeError(f"GitHub operation failed: gh {' '.join(args[:2])}; check authentication and repository access.")
    return result.stdout.strip()


def remote_state() -> tuple[bool, bool]:
    secrets = json.loads(github("secret", "list", "--json", "name"))
    variables = json.loads(github("variable", "list", "--json", "name"))
    return any(v["name"] == SECRET for v in secrets), any(v["name"] == VARIABLE for v in variables)


def validate_public_key(value: str) -> str:
    value = value.strip().lower()
    if not re.fullmatch(r"[0-9a-f]{64}", value) or int(value, 16) == 0:
        raise RuntimeError("The configured public key is invalid; no existing value was changed.")
    return value


def save_public(value: str) -> None:
    if PUBLIC_FILE.exists() and PUBLIC_FILE.read_text().strip().lower() != value:
        raise RuntimeError("The local public key differs from GitHub; refusing to overwrite it.")
    PUBLIC_FILE.parent.mkdir(parents=True, exist_ok=True)
    PUBLIC_FILE.write_text(value + "\n")


def configure(signer: str) -> None:
    existing_secret, existing_variable = remote_state()
    if existing_secret and existing_variable:
        save_public(validate_public_key(github("variable", "get", VARIABLE)))
        print("Signing already configured; existing repository values preserved.")
        return
    if existing_secret != existing_variable:
        raise RuntimeError("Incomplete existing signing configuration. No value was changed; recover the matching missing value manually.")
    if PUBLIC_FILE.exists():
        raise RuntimeError("A local public key already exists but GitHub configuration is absent. Refusing an implicit key rotation.")
    if not Path(signer).is_file() and shutil.which(signer) is None:
        raise RuntimeError("zipsign is missing: cargo install zipsign --version 0.2.2 --locked --root .tools")

    with tempfile.TemporaryDirectory(prefix="garage-update-key-") as directory:
        private = Path(directory) / "private.key"
        public = Path(directory) / "public.key"
        subprocess.run([signer, "gen-key", str(private), str(public)], check=True, capture_output=True)
        private.chmod(0o600)
        key = validate_public_key(public.read_bytes().hex())
        encoded = base64.b64encode(private.read_bytes()).decode("ascii")
        # Recheck immediately before the first write. gh set is an upsert, so
        # this also catches another maintainer configuring keys during generation.
        if remote_state() != (False, False):
            raise RuntimeError("Signing configuration changed concurrently; no generated value was uploaded.")
        github("secret", "set", SECRET, stdin=encoded)
        del encoded
        # Preserve only the public half, including after a partial network failure.
        # It can recover a failed variable write without replacing the secret.
        save_public(key)
        github("variable", "set", VARIABLE, "--body", key)
    if remote_state() != (True, True):
        raise RuntimeError("Could not verify both GitHub entries after configuration; inspect the repository settings.")
    if validate_public_key(github("variable", "get", VARIABLE)) != key:
        raise RuntimeError("GitHub public key verification failed; inspect the repository settings.")
    print("Signed updates configured. Private key stored only in GitHub Actions; public key saved to assets/update-public-key.txt.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zipsign", default=str(Path(__file__).resolve().parents[1] / ".tools" / "bin" / "zipsign"))
    options = parser.parse_args()
    try:
        configure(options.zipsign)
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"Configuration stopped: {error}\n")
