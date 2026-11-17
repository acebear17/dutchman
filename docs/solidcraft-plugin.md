# Dutchman in SolidCraft

The Dutchman plugin lets SolidCraft open Fusion (`.f3d`, `.f3z`), Inventor (`.ipt`, `.iam`), and
SolidWorks (`.sldprt`, `.sldasm`) files directly. File > Open converts the file with the same code
as the `dutchman` command and opens the result as a new, unsaved design with its editable history.
SolidCraft's Import report window lists only what the design lost, one line per kind of loss;
notes about how the file was read stay under **Details**.

FreeCAD files (`.FCStd`) are not among the plugin's formats: SolidCraft opens them itself with
**File > Open**, and its own reader keeps more of their editable history than the plugin does
today. On the public `design_history.FCStd` test file, SolidCraft's reader keeps all five
features editable; through Dutchman the replayed Pocket cuts away from the body instead of into
it, so SolidCraft's check refuses the history and only the shape would come across. The
`dutchman` command still converts FreeCAD files.

It needs SolidCraft 0.44 or newer, on the desktop. SolidCraft's browser version does not run
plugins yet. Older SolidCraft releases refuse to install it: they cannot run importers or grant
the input folder it reads assembly parts from.

## Install

1. Download [`dutchman-plugin.zip`](https://github.com/dutchman-cad/dutchman/releases/latest/download/dutchman-plugin.zip)
   from the latest release ([checksum](https://github.com/dutchman-cad/dutchman/releases/latest/download/dutchman-plugin.zip.sha256)).
   The [release page](https://github.com/dutchman-cad/dutchman/releases/latest) also lists the
   same file as `dutchman-plugin-X.Y.Z.zip`, with `dutchman-plugin-X.Y.Z.zip.sha256`.
2. In SolidCraft, open **Preferences > Plugins** and choose **Install plugin...**, then pick the
   zip as it is. Don't unzip it: SolidCraft installs the zip, not the files inside.
3. Read the confirmation screen: it shows the plugin's name, version, and what it asks for (see
   below):
   - **2048 MiB of memory**, above SolidCraft's usual 128 MiB plugin limit;
   - **read access to the input folder**, the folder of each file you open, for an assembly's
     parts.

   **Install** approves both; **Cancel** changes nothing.

Installing never runs the plugin. After that, **File > Open** offers the Dutchman file types, and
Dutchman is listed in the Plugins menu, where its dialog shows the options before it asks for the
file. To update, install the newer zip over the old one; to remove it, use **Remove** on its card
in **Preferences > Plugins**. Designs you made with it stay as they are.

SolidCraft checks the converted history against the original geometry. A history that matches
is the visible model; one it cannot check stays hidden beside the original geometry, marked as
not checked.

## What it asks for

| Asks for | Why |
|---|---|
| Read access to the folder of the file you open | An assembly names its part files; the plugin reads them from that folder. Without it, only the file you open is read. |
| 2048 MiB of memory | Real CAD files need it; see below. |

It has no network access, no clock or settings access, and no permission to write files. It
cannot change the design you have open: it only produces a new one.

## The memory ceiling

SolidCraft limits a plugin to 128 MiB unless its manifest asks for more and you approve that at
install. Dutchman asks for 2048 MiB, because converting a Fusion, Inventor, or SolidWorks
file keeps the whole decoded model in memory, not only the file's bytes. The install
screen says how much it asks for and what your ceiling is; **Cancel** installs nothing. The figure
is a limit for one conversion, not memory the plugin holds on to between files.

Measured: the public test files (5 to 15 KB each) need 3 to 15 MiB inside the plugin. Real
designs are far larger, and memory grows with the design's size and its number of features, not
the file size alone. A 68 MB Fusion archive with a long history needed about 3.8 GiB on the command
line, which is more than the plugin can have. That file used about 57 times its size, so
files up to roughly 30 MB should fit in 2048 MiB. A bigger one stops with a memory-limit error, and
the `dutchman` command line, which has no such limit, converts it instead.

## Options

The dialog shows three options:

- **Keep editable history** (on): keep the features Dutchman could map next to the original
  geometry. Turn it off for geometry only.
- **Up axis**: **As in the file** keeps the file's coordinates. **Stand up (Y-up to Z-up)**
  turns a Y-up design to Z-up.
- **Include hidden bodies** (off): also bring in hidden or consumed bodies.

## Limits

- SolidCraft 0.44 or newer, desktop only for now.
- One file per conversion, at most 1 GiB read (the file and its assembly parts together) and
  256 MiB written. A bigger file stops with a message saying so; the `dutchman` command line,
  which has no such limit, converts it instead.
- Fusion, Inventor, and SolidWorks only. FreeCAD files open with SolidCraft's own reader (see
  above); for any other format the plugin's message names the formats it converts.

## When a conversion stops

The message says what to do first, then the detail:

- **A file it can't read** (incomplete, or not really the format its extension says): download
  or copy it again and open it once more. If the original opens in the program that made it,
  report it on the [A file didn't convert](https://github.com/dutchman-cad/dutchman/issues/new?template=file-did-not-convert.yml)
  form; `dutchman convert FILE --report-anon report.json` makes an anonymous report to attach,
  with no design data in it.
- **Over 1 GiB, or over the memory limit**: convert it with the `dutchman` command line.
- **Missing assembly parts**: keep the part files in the assembly's folder, under their saved
  names, and open the assembly again.

For how the plugin is built and tested, see [the plugin notes](plugin.md).
