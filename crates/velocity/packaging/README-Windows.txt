Velocity for Windows x64
========================

Windows 10/11 x64 and a DirectX 12-capable graphics driver are required.
Extract the entire ZIP and open velocity.exe, or run the optional per-user
installer. No administrator permission is needed. Both distributions use
%APPDATA%\Velocity for your profile, not the application directory.

These builds are unsigned. Verify the ZIP or installer against SHA256SUMS on
the release page before opening it. For example, in PowerShell:

    Get-FileHash -Algorithm SHA256 .\Velocity-*-windows-x64.zip

SmartScreen may warn about an unrecognized publisher. Continue only for an
artifact you have verified and trust, and only if permitted by your policy.
Do not disable SmartScreen system-wide.

In Command Prompt or PowerShell, use vel.exe:

    vel.exe file.txt:12:3
    vel.exe --wait file.txt
    Get-Content notes.txt | vel.exe - | Set-Content edited-notes.txt

Use the full path to vel.exe, run it from the extracted folder (PowerShell:
.\vel.exe), or add the folder to your user PATH. The installer can add it for
you; open a new terminal after changing PATH. Keep vel.exe and velocity.exe
together. Close the requested sheets to finish --wait or return edited stdin.

Key bindings: Preferences > Key Bindings opens Default (Windows).sublime-keymap.
Release and third-party licenses are in Resources.
