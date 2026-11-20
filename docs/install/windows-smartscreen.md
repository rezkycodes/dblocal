# Windows SmartScreen: "Windows protected your PC" / "Unknown publisher"

DBLocal for Windows is **unsigned in v1** (D-20). We do not currently hold a code-signing
certificate, so Windows cannot identify the publisher and **Microsoft Defender SmartScreen** will
show a warning the first time you run the downloaded app.

This document explains what the prompt means, what to do, and how to verify the download yourself.
Nothing here asks you to trust us; the steps below let you **verify the bits**.

> **Never ignore a SmartScreen prompt blindly.** If you did not download the file from the official
> DBLocal GitHub release page for the version you intended to install, delete it.

## What you will see

When you open the `.zip`/extracted binary (or a future installer), Windows typically shows:

- **"Windows protected your PC"** (SmartScreen) with the text  
  *"Microsoft Defender SmartScreen prevented an unrecognized app from starting. Running this app
  might put your PC at risk."*  
  and the publisher shown as **Unknown publisher**.
- Sometimes a secondary dialog: *"Do you want to allow this app from an unknown publisher to make
  changes to your device?"* (User Account Control / execution prompt).

These warnings appear because the binary is not signed with a certificate that chains to a trusted
publisher — **not** because SmartScreen has identified this specific file as malware.

## Before you click anything: verify the checksum

Because the publisher is unknown, your safety check is the **SHA-256 checksum** published with the
release. Do this on the file you downloaded, from the **same** GitHub release.

1. Open the release on GitHub: `https://github.com/rezkycodes/dblocal/releases/tag/<version>`
   and download **both** the Windows artifact
   (`dblocal-<version>-windows-x86_64.zip`) and **`checksums.txt`**.
2. Compute the hash of the file you downloaded:

   **PowerShell (recommended):**
   ```powershell
   Get-FileHash .\dblocal-<version>-windows-x86_64.zip -Algorithm SHA256
   ```

   **Command Prompt (`certutil`):**
   ```bat
   certutil -hashfile dblocal-<version>-windows-x86_64.zip SHA256
   ```

3. Open `checksums.txt` from the same release and find the line for
   `dblocal-<version>-windows-x86_64.zip`. The 64-character hex hash must match **exactly** (case
   does not matter). If it does **not** match, delete the file and re-download from the official
   release; if it still does not match, do **not** run it and report the mismatch in a GitHub issue.

4. (Optional, stronger) If you have the GitHub CLI installed, you can also verify the release's
   **build provenance attestation** (proves the artifact was built from this repository by the
   release workflow):
   ```powershell
   gh attestation verify .\dblocal-<version>-windows-x86_64.zip --repo rezkycodes/dblocal
   ```

Only proceed once the checksum matches the official `checksums.txt`.

## How to proceed past the SmartScreen prompt

1. In the **"Windows protected your PC"** dialog, click **"More info"**.
2. A **"Run anyway"** button appears (it only shows after "More info"). Click **"Run anyway"** to
   start DBLocal.
3. If a User Account Control prompt appears asking to allow changes, read it carefully: DBLocal does
   **not** require administrator rights and never asks for elevation at runtime. If a prompt asks for
   admin/elevation in a way that seems unrelated to simply running the app you verified, decline and
   investigate.

After the first successful run, SmartScreen may stop warning for that specific file.

## Why it is unsigned (and what would change)

Code-signing certificates (EV or standard Authenticode) cost money and require identity verification
and secure key storage. As a free, pre-alpha, no-telemetry project, DBLocal does not yet carry that
cost. When/if a certificate is obtained:

- the release workflow will sign the Windows artifact,
- this document will be updated, and
- the "Unknown publisher" prompt will go away once the certificate builds reputation (or immediately
  with an EV certificate).

Until then, **checksum verification is the trust mechanism**.

## If you are not comfortable

- Use the **Linux or macOS** build if you have access to those platforms (macOS releases are
  signed and notarized when Apple Developer ID secrets are configured).
- Wait for a signed Windows release.
- Build from source yourself from the tagged commit and compare results.

## Reporting problems

If the checksum does not match, the download location looks wrong, or the app behaves in a way that
does not match this document, open an issue at
`https://github.com/rezkycodes/dblocal/issues` and include the version/tag, the hash you computed,
and where you downloaded the file from. **Do not include passwords or the contents of `~/.dblocal`.**
