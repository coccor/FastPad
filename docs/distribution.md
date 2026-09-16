# Distribution

FastPad ships one build in two forms, attached to every GitHub release:

| Asset | Built by | Used by |
|---|---|---|
| `FastPad-<version>-windows-x64.zip` | `tools/package.ps1` | Portable download, Scoop |
| `FastPad-<version>-windows-x64-setup.exe` | `tools/package-installer.ps1` (Inno Setup 6) | Direct download, winget |
| `SHA256SUMS.txt` | `release.yml` | Scoop and winget manifests, users |

`Cargo.toml` is the only place the version is written.

## Cutting a release

1. Bump `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` follows, and merge to `main`.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. `release.yml` runs the full CI suite, checks the tag equals `v` + the `Cargo.toml` version, builds
   and verifies the ZIP and installer (signing both when configured), and creates a **draft** release
   with generated notes. A version with a hyphen (`0.2.0-rc.1`) is marked as a pre-release.
4. Review the draft on GitHub and publish it.
5. `publish-manifests.yml` runs on publish (skipped for pre-releases):
   - opens a pull request updating `bucket/fastpad.json` (merge it to update Scoop users);
   - submits the winget manifest to `microsoft/winget-pkgs` if `WINGET_TOKEN` is set, and always
     uploads the rendered manifests as a workflow artifact.

   Re-run it for an already-published tag from Actions > publish-manifests > Run workflow.

## One-time repository setup

- **Allow Actions to open pull requests:** Settings > Actions > General > Workflow permissions >
  check "Allow GitHub Actions to create and approve pull requests". Without it the Scoop step fails.
- **winget (optional automation):** create a classic personal access token with the `public_repo`
  scope and save it as the repository secret `WINGET_TOKEN`. `wingetcreate` forks
  `microsoft/winget-pkgs` under that account and opens the PR. The first submission of
  `coccor.FastPad` gets a manual moderator review; later versions are usually merged automatically.
  Without the token, download the `manifests-<tag>` artifact and run
  `wingetcreate submit --token <token> dist/winget/<version>` yourself.

## Scoop

This repository is the bucket: Scoop reads `bucket/*.json` from any Git repository.

```powershell
scoop bucket add fastpad https://github.com/coccor/FastPad
scoop install fastpad/fastpad
```

The manifest installs the portable ZIP, creates a `fastpad` shim and a Start menu shortcut, and
carries `checkver`/`autoupdate` so `scoop update` and Scoop's own tooling can follow new releases.

## winget

Package identifier `coccor.FastPad`, installer type `inno`, scope `user`. The Inno Setup `AppId`
(`{DD2BB0BB-6510-4E72-922F-75CC10D1244E}`) is the winget product code (`{AppId}_is1`), so it must never
change or winget will treat upgrades as a different product. Check a rendered manifest locally with
`winget validate --manifest dist/winget/<version>`.

## Code signing

Unsigned builds work but trigger SmartScreen ("Windows protected your PC") on download and reach
winget's reviewers flagged as unsigned. `tools/signing.ps1` signs `FastPad.exe`, both DLLs, the
installer, and its uninstaller when either mode below is configured; `verify-package.ps1` and
`verify-installer.ps1` then require valid signatures in CI.

### Azure Artifact Signing (recommended, used by CI)

Formerly Trusted Signing: Microsoft-managed certificates for roughly USD 10 a month, trusted by
SmartScreen without an EV certificate. Check current eligibility for individual developers in your
country before signing up.

1. In Azure, create an Artifact Signing account, complete identity validation, and create a
   certificate profile (Public Trust).
2. Create an app registration (service principal) with a client secret, and give it the
   *Artifact Signing Certificate Profile Signer* role on the account.
3. In the GitHub repository add:
   - variables `ARTIFACT_SIGNING_ENDPOINT` (the account's region endpoint, for example
     `https://weu.codesigning.azure.net`), `ARTIFACT_SIGNING_ACCOUNT`, `ARTIFACT_SIGNING_PROFILE`;
   - secrets `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_CLIENT_SECRET`.

`release.yml` enables signing as soon as `ARTIFACT_SIGNING_ACCOUNT` is set. To sign locally, download
the `Microsoft.ArtifactSigning.Client` NuGet package, `az login`, and set `FASTPAD_SIGNING_DLIB` to its
`bin\x64\Azure.CodeSigning.Dlib.dll` and `FASTPAD_SIGNING_METADATA` to a JSON file with `Endpoint`,
`CodeSigningAccountName`, and `CertificateProfileName`.

### Local certificate

Set `FASTPAD_SIGNING_CERTIFICATE` to a certificate thumbprint in the current user's store (for
example an OV certificate on a hardware token) or a PFX path, with `FASTPAD_SIGNING_PASSWORD` and
`FASTPAD_TIMESTAMP_URL` as needed, then run `tools/package.ps1` and `tools/package-installer.ps1`.
