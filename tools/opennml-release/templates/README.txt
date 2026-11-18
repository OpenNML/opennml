OpenNML v{version}
==================

OpenNML is a faithful reimplementation of the real-time strategy game
"No Man's Land" (2003) in Rust. It is free software.

  Version:     OpenNML v{version}
  Commit:      {commit}
  Source code: https://github.com/OpenNML/opennml


Requirements
------------

- Your own original installation of "No Man's Land" (German or English
  edition). The reference is the German edition with the patch. OpenNML
  contains no game data; it reads the files from your installation.
- Windows 10 or 11, 64 bit.
- A graphics card with DirectX 12 or Vulkan support.


Starting
--------

1. Unpack the ZIP file to any folder.
2. Double-click {exe}.

OpenNML looks for the original installation on its own: next to the program,
in the Steam libraries, in the registry and in the usual program folders. If
it finds nothing, it asks for the folder of "No Man's Land" in a dialog. The
folder you choose is remembered.

For special cases you can name the folder yourself: start {exe} with
--game-dir "C:\Path\To\No Man's Land" (for example from a shortcut or from a
command prompt). The installation itself is never written to.


Windows SmartScreen
-------------------

The program is not signed. Windows SmartScreen may therefore show "Windows
protected your PC". Click "More info", then "Run anyway".

To check the download, compare the SHA-256 checksum with the .sha256 file
that belongs to the release. In PowerShell:

  Get-FileHash .\{exe} -Algorithm SHA256

The ZIP file itself can be checked the same way against its .sha256 file.


Firewall (network play)
-----------------------

For games on a local network Windows may ask whether to allow {exe}. The
program uses TCP port 2302 and UDP port 2303.


Where OpenNML stores its files
------------------------------

Settings, profiles, savegames and the log file are stored in

  %LOCALAPPDATA%\OpenNML\

The log file is logs\OpenNML.log in that folder (the previous run is kept as
logs\OpenNML.1.log). If the program reports an error or something does not
work, please attach the log file to your report.


Removing
--------

Delete the folder you unpacked the ZIP file to, and the folder
%LOCALAPPDATA%\OpenNML\ if you do not need your settings and savegames any
more. The original installation is not touched.


Legal note
----------

OpenNML is not an official product. "No Man's Land" is a game by Related
Designs, published by CDV Software; the names and trademarks belong to their
owners. This project is not affiliated with the rights holders.

You need your own original installation to play. OpenNML contains no game
files, graphics, sounds or texts of the original.

OpenNML is licensed under the GNU General Public License, version 3 or later;
see LICENSE.txt. The video decoder is under the LGPL 2.1 or later, the
documentation and artwork under CC BY-SA 4.0, and the third-party libraries
under their own licenses. All license texts and the list of third-party
libraries are in the folder licenses\ (see licenses\THIRD-PARTY.txt).


Source code
-----------

The source code of this version is available at
https://github.com/OpenNML/opennml, and as the archive "Source code" attached
to this release (GPL-3.0 section 6).
