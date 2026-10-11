# winget package

Manifests for publishing PC Hub to winget (`winget install SudeepMishra.PCHub`).

## First release

winget only accepts packages whose installer is already downloadable, so publish a GitHub
release first (push a tag such as `v0.1.0`; the Release workflow builds the installer).
Then:

1. Download `pc-hub-setup-0.1.0.exe` from the release and get its hash:
   `(Get-FileHash pc-hub-setup-0.1.0.exe).Hash`
2. Put the hash in `manifests/s/SudeepMishra/PCHub/0.1.0/SudeepMishra.PCHub.installer.yaml`
   (and change the version in all three files if it isn't 0.1.0).
3. Check them: `winget validate --manifest winget\manifests\s\SudeepMishra\PCHub\0.1.0`
4. Try them: `winget install --manifest winget\manifests\s\SudeepMishra\PCHub\0.1.0`
5. Submit: fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs), copy the
   folder to the same path there, and open a pull request. Or let
   [wingetcreate](https://github.com/microsoft/winget-create) do it:
   `wingetcreate submit winget\manifests\s\SudeepMishra\PCHub\0.1.0`

## Later releases

Once the package is in winget-pkgs, the Release workflow can open the update pull
request by itself: create a classic GitHub token with the `public_repo` scope and add it
to this repository's secrets as `WINGET_TOKEN`. Without that secret the step is skipped.
