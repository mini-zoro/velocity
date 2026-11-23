using namespace System.Windows.Automation

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName @('System.Drawing', 'UIAutomationClient', 'UIAutomationTypes')
if (-not ('VelocityDesktop.Native' -as [type])) {
    Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

namespace VelocityDesktop {
    public static class Native {
        [StructLayout(LayoutKind.Sequential)] struct Rect {
            public int L, T, R, B;
            public Rectangle Area { get { return Rectangle.FromLTRB(L, T, R, B); } }
        }
        [StructLayout(LayoutKind.Sequential)] struct Point { public int X, Y; }
        [StructLayout(LayoutKind.Sequential)] struct GuiThread {
            public uint Size, Flags;
            public IntPtr Active, Focus, Capture, MenuOwner, MoveSize, Caret;
            public Rect CaretRect;
        }
        [StructLayout(LayoutKind.Sequential)] struct MenuBarInfo {
            public uint Size;
            public Rect Bounds;
            public IntPtr Menu, Popup;
            public uint Flags;
        }
        [StructLayout(LayoutKind.Sequential)] struct Mouse {
            public int X, Y;
            public uint Data, Flags, Time;
            public UIntPtr Extra;
        }
        [StructLayout(LayoutKind.Sequential)] struct Keyboard {
            public ushort Vk, Scan;
            public uint Flags, Time;
            public UIntPtr Extra;
        }
        [StructLayout(LayoutKind.Explicit)] struct InputData {
            [FieldOffset(0)] public Mouse Mouse;
            [FieldOffset(0)] public Keyboard Key;
        }
        [StructLayout(LayoutKind.Sequential)] struct Input {
            public uint Type;
            public InputData Data;
        }
        delegate bool WindowCallback(IntPtr hwnd, IntPtr data);

        [DllImport("user32.dll")] static extern bool EnumWindows(WindowCallback callback, IntPtr data);
        [DllImport("user32.dll")] static extern bool EnumThreadWindows(uint thread, WindowCallback callback, IntPtr data);
        [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool IsIconic(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool IsChild(IntPtr parent, IntPtr child);
        [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
        [DllImport("user32.dll")] static extern int GetDlgCtrlID(IntPtr control);
        [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr dialog, int controlId);
        [DllImport("user32.dll")] static extern bool ShowWindowAsync(IntPtr hwnd, int command);
        [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder value, int length);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder value, int length);
        [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from, uint to, bool attach);
        [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(uint thread, ref GuiThread info);
        [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
        [DllImport("user32.dll")] static extern IntPtr GetThreadDesktop(uint thread);
        [DllImport("user32.dll", SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
        [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
        [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        static extern bool GetUserObjectInformation(IntPtr obj, int index, StringBuilder value, int bytes, out int needed);
        [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
        [DllImport("user32.dll")] static extern IntPtr GetWindowDpiAwarenessContext(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool AreDpiAwarenessContextsEqual(IntPtr first, IntPtr second);
        [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
        [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
        [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
        [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr hwnd, int attribute, out Rect rect, int bytes);
        [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr hwnd, out Rect rect);
        [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr hwnd, ref Point point);
        [DllImport("user32.dll", SetLastError=true)]
        static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int w, int h, uint flags);
        [DllImport("user32.dll")] static extern int GetWindowLongW(IntPtr hwnd, int index);
        [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr dest, uint flags);
        [DllImport("user32.dll")] static extern IntPtr SendMessageTimeoutW(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp, uint flags, uint ms, out UIntPtr result);
        [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageTimeoutW", ExactSpelling=true)]
        static extern IntPtr GetTextMessage(IntPtr hwnd, uint msg, IntPtr wp, StringBuilder lp, uint flags, uint ms, out UIntPtr result);
        [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count, Input[] input, int size);
        [DllImport("user32.dll")] static extern uint MapVirtualKeyW(uint vk, uint mode);
        [DllImport("user32.dll", SetLastError=true)] static extern bool SetCursorPos(int x, int y);
        [DllImport("user32.dll")] static extern IntPtr GetKeyboardLayout(uint thread);
        [DllImport("user32.dll")] static extern IntPtr GetMenu(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool GetMenuBarInfo(IntPtr hwnd, int objectId, int itemId, ref MenuBarInfo info);
        [DllImport("user32.dll")] static extern bool GetMenuItemRect(IntPtr hwnd, IntPtr menu, uint index, out Rect rect);
        [DllImport("user32.dll")] static extern uint GetMenuState(IntPtr menu, uint index, uint flags);
        [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();
        [DllImport("user32.dll")] public static extern IntPtr GetClipboardOwner();
        [DllImport("user32.dll")] public static extern IntPtr GetOpenClipboardWindow();
        [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr menu, int index);
        [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr menu);
        [DllImport("user32.dll", CharSet=CharSet.Unicode)]
        static extern int GetMenuString(IntPtr menu, uint index, StringBuilder label, int length, uint flags);

        public static uint Pid(IntPtr hwnd) { uint pid; GetWindowThreadProcessId(hwnd, out pid); return pid; }
        static uint ThreadOf(IntPtr hwnd) { uint pid; return GetWindowThreadProcessId(hwnd, out pid); }
        public static string Title(IntPtr hwnd) {
            var value = new StringBuilder(2048); GetWindowText(hwnd, value, value.Capacity); return value.ToString();
        }
        public static string Class(IntPtr hwnd) {
            var value = new StringBuilder(256); GetClassName(hwnd, value, value.Capacity); return value.ToString();
        }
        public static string Describe(IntPtr hwnd) {
            if (hwnd == IntPtr.Zero) return "(no window)";
            return string.Format("HWND=0x{0:X} PID={1} class={2} title='{3}'", hwnd.ToInt64(), Pid(hwnd), Class(hwnd), Title(hwnd));
        }
        public static IntPtr[] Windows(uint pid) {
            var list = new List<IntPtr>();
            EnumWindows(delegate(IntPtr h, IntPtr _) {
                if (IsWindowVisible(h) && !IsIconic(h) && (pid == 0 || Pid(h) == pid)) list.Add(h);
                return true;
            }, IntPtr.Zero);
            return list.ToArray();
        }
        public static IntPtr[] Menus(IntPtr owner) {
            var list = new List<IntPtr>();
            EnumThreadWindows(ThreadOf(owner), delegate(IntPtr h, IntPtr _) {
                if (IsWindowVisible(h) && Class(h) == "#32768") list.Add(h);
                return true;
            }, IntPtr.Zero);
            return list.ToArray();
        }
        public static IntPtr Foreground { get { return GetForegroundWindow(); } }
        public static void Focus(IntPtr hwnd) {
            if (IsIconic(hwnd)) ShowWindowAsync(hwnd, 9);
            if (Foreground == hwnd || SetForegroundWindow(hwnd)) return;
            uint here = GetCurrentThreadId(), active = ThreadOf(Foreground), target = ThreadOf(hwnd);
            bool attached = active != 0 && active != here && AttachThreadInput(here, active, true);
            bool toTarget = target != 0 && target != here && target != active && AttachThreadInput(here, target, true);
            try { SetForegroundWindow(hwnd); }
            finally {
                if (toTarget) AttachThreadInput(here, target, false);
                if (attached) AttachThreadInput(here, active, false);
            }
        }
        public static bool InMenu(IntPtr hwnd) {
            var info = new GuiThread { Size = (uint)Marshal.SizeOf(typeof(GuiThread)) };
            uint thread = ThreadOf(hwnd);
            return thread != 0 && GetGUIThreadInfo(thread, ref info) && info.MenuOwner == hwnd && (info.Flags & 4) != 0;
        }
        public static bool OwnedBy(IntPtr popup, IntPtr owner) { return GetAncestor(popup, 3) == owner; }
        public static IntPtr FilenameFocus(IntPtr dialog) {
            var info = new GuiThread { Size = (uint)Marshal.SizeOf(typeof(GuiThread)) };
            if (!GetGUIThreadInfo(ThreadOf(dialog), ref info) || !IsChild(dialog, info.Focus)) return IntPtr.Zero;
            // The common item dialog uses an Edit within the file-name shell combo.
            return Class(info.Focus) == "Edit" || GetDlgCtrlID(info.Focus) == 1001 ? info.Focus : IntPtr.Zero;
        }
        public static string ControlText(IntPtr hwnd) {
            var value = new StringBuilder(32768); UIntPtr ignored;
            return GetTextMessage(hwnd, 0x000d, new IntPtr(value.Capacity), value, 2, 500, out ignored) == IntPtr.Zero
                ? null : value.ToString();
        }
        public static bool Responding(IntPtr hwnd) {
            UIntPtr ignored;
            return SendMessageTimeoutW(hwnd, 0, IntPtr.Zero, IntPtr.Zero, 2, 250, out ignored) != IntPtr.Zero;
        }
        public static IntPtr MenuBar(IntPtr hwnd) { return GetMenu(hwnd); }
        public static IntPtr MenuPopup(IntPtr hwnd, int index) {
            var info = new MenuBarInfo { Size = (uint)Marshal.SizeOf(typeof(MenuBarInfo)) };
            if (GetMenuBarInfo(hwnd, -3, index + 1, ref info) && info.Popup != IntPtr.Zero && IsWindowVisible(info.Popup))
                return info.Popup;
            // Some builds do not expose the popup HWND through GetMenuBarInfo.
            // Compare the displayed submenu's item geometry with the actual popup.
            IntPtr branch = GetSubMenu(GetMenu(hwnd), index);
            Rectangle row = MenuItemArea(IntPtr.Zero, branch, 0);
            if (!row.IsEmpty)
                foreach (IntPtr popup in Menus(hwnd))
                    if (Outer(popup).Contains(row.Location)) return popup;
            return IntPtr.Zero;
        }
        public static Rectangle MenuItemArea(IntPtr hwnd, IntPtr menu, uint index) {
            Rect rect;
            return menu != IntPtr.Zero && GetMenuItemRect(hwnd, menu, index, out rect) ? rect.Area : Rectangle.Empty;
        }
        public static bool MenuItemEnabled(IntPtr menu, uint index) { return (GetMenuState(menu, index, 0x400) & 3) == 0; }
        public static int HighlightedMenuRow(IntPtr menu) {
            int count = GetMenuItemCount(menu);
            for (int i = 0; i < count; ++i) {
                uint state = GetMenuState(menu, (uint)i, 0x400); // MF_BYPOSITION
                if (state != uint.MaxValue && (state & 0x80) != 0) return i; // MF_HILITE
            }
            return -1;
        }
        public static string MenuLabel(IntPtr menu, uint index) {
            var value = new StringBuilder(1024);
            GetMenuString(menu, index, value, value.Capacity, 0x400);
            return value.ToString();
        }
        static string DesktopName(IntPtr desktop) {
            int needed;
            var value = new StringBuilder(256);
            if (desktop == IntPtr.Zero || !GetUserObjectInformation(desktop, 2, value, 512, out needed))
                throw new Win32Exception();
            return value.ToString();
        }
        public static string InputDesktop() {
            IntPtr input = OpenInputDesktop(0, false, 1); // DESKTOP_READOBJECTS
            if (input == IntPtr.Zero) throw new Win32Exception();
            try {
                string ours = DesktopName(GetThreadDesktop(GetCurrentThreadId())), active = DesktopName(input);
                if (ours != active) throw new InvalidOperationException("Desktop is " + ours + ", active desktop is " + active);
                SetThreadDpiAwarenessContext(new IntPtr(-4));
                return active;
            } finally { CloseDesktop(input); }
        }
        public static string Awareness(IntPtr hwnd) {
            IntPtr context = GetWindowDpiAwarenessContext(hwnd);
            string[] names = { "Unaware", "System", "PerMonitor", "PerMonitorV2", "UnawareGdiScaled" };
            for (int i = 0; i < names.Length; ++i)
                if (AreDpiAwarenessContextsEqual(context, new IntPtr(-1-i))) return names[i];
            return "Unknown";
        }
        public static Rectangle DesktopBounds() {
            return new Rectangle(GetSystemMetrics(76), GetSystemMetrics(77), GetSystemMetrics(78), GetSystemMetrics(79));
        }
        public static Rectangle Outer(IntPtr hwnd) {
            Rect rect; return GetWindowRect(hwnd, out rect) ? rect.Area : Rectangle.Empty;
        }
        public static Rectangle VisibleBounds(IntPtr hwnd) {
            Rect rect;
            if (DwmGetWindowAttribute(hwnd, 9, out rect, Marshal.SizeOf(typeof(Rect))) == 0 && !rect.Area.IsEmpty)
                return rect.Area;
            return Outer(hwnd);
        }
        public static Rectangle Client(IntPtr hwnd) {
            var at = new Point(); Rect rect;
            if (!GetClientRect(hwnd, out rect) || !ClientToScreen(hwnd, ref at)) return Rectangle.Empty;
            return new Rectangle(at.X, at.Y, rect.R, rect.B);
        }
        public static void MoveResize(IntPtr hwnd, int x, int y, int width, int height) {
            if (!SetWindowPos(hwnd, IntPtr.Zero, x, y, width, height, 0x14)) throw new Win32Exception();
        }
        public static bool HasCaption(IntPtr hwnd) { return (GetWindowLongW(hwnd, -16) & 0xc00000) == 0xc00000; }
        public static int InputLanguage(IntPtr hwnd) { return (int)(GetKeyboardLayout(ThreadOf(hwnd)).ToInt64() & 0xffff); }

        static Input Key(ushort scan, uint flags) {
            return new Input { Type=1, Data=new InputData { Key=new Keyboard { Scan=scan, Flags=flags } } };
        }
        static void Send(Input[] input) {
            if (input.Length != 0 && SendInput((uint)input.Length, input, Marshal.SizeOf(typeof(Input))) != input.Length)
                throw new Win32Exception("SendInput was blocked or only partially accepted");
        }
        public static void Chord(uint[] keys) {
            var input = new Input[keys.Length * 2];
            for (int i = 0; i < keys.Length; ++i) {
                uint scan = MapVirtualKeyW(keys[i], 4); // MAPVK_VK_TO_VSC_EX includes E0/E1
                if (scan == 0) throw new ArgumentException("No scan code for VK " + keys[i]);
                uint flags = (scan & 0xff00) == 0xe000 ? 9u : 8u;
                input[i] = Key((ushort)(scan & 255), flags);
                input[input.Length - 1 - i] = Key((ushort)(scan & 255), flags | 2);
            }
            Send(input);
        }
        public static void Text(string text) {
            // Send each UTF-16 unit, including both halves of a surrogate pair.
            var input = new List<Input>();
            foreach (char unit in text) { input.Add(Key(unit, 4)); input.Add(Key(unit, 6)); }
            Send(input.ToArray());
        }
        public static void Click(int x, int y) {
            if (!SetCursorPos(x, y)) throw new Win32Exception();
            Send(new [] {
                new Input { Data=new InputData { Mouse=new Mouse { Flags=2 } } },
                new Input { Data=new InputData { Mouse=new Mouse { Flags=4 } } }
            });
        }
        public static Bitmap Screen(Rectangle area) {
            var bmp = new Bitmap(area.Width, area.Height, PixelFormat.Format32bppRgb);
            try {
                using (var g = Graphics.FromImage(bmp))
                    g.CopyFromScreen(area.Location, System.Drawing.Point.Empty, area.Size, CopyPixelOperation.SourceCopy);
                return bmp;
            } catch { bmp.Dispose(); throw; }
        }
        public static Bitmap WindowImage(IntPtr hwnd) {
            var area = Outer(hwnd);
            if (area.IsEmpty || !Responding(hwnd)) throw new InvalidOperationException("Window not responding to capture");
            var bmp = new Bitmap(area.Width, area.Height, PixelFormat.Format32bppRgb);
            try {
                using (var g = Graphics.FromImage(bmp)) {
                    IntPtr dc = g.GetHdc();
                    try { if (!PrintWindow(hwnd, dc, 2)) throw new Win32Exception("PrintWindow failed"); }
                    finally { g.ReleaseHdc(dc); }
                }
                return bmp;
            } catch { bmp.Dispose(); throw; }
        }
        static int[] Pixels(Bitmap image) {
            var data = image.LockBits(new Rectangle(0, 0, image.Width, image.Height), ImageLockMode.ReadOnly, PixelFormat.Format32bppRgb);
            try {
                var pixels = new int[image.Width * image.Height];
                for (int y = 0; y < image.Height; ++y)
                    Marshal.Copy(IntPtr.Add(data.Scan0, y * data.Stride), pixels, y * image.Width, image.Width);
                return pixels;
            } finally { image.UnlockBits(data); }
        }
        // Most frequent color is a synthetic blank frame for the captured region.
        public static Dictionary<int, int> Histogram(Bitmap image) {
            var counts = new Dictionary<int, int>();
            foreach (int pixel in Pixels(image)) {
                int rgb = pixel & 0xffffff, count;
                counts.TryGetValue(rgb, out count); counts[rgb] = count + 1;
            }
            return counts;
        }
        public static int Difference(Bitmap a, Bitmap b) {
            return Difference(a, b, 24);
        }
        public static int Difference(Bitmap a, Bitmap b, int tolerance) {
            if (a.Size != b.Size) throw new ArgumentException("Frame comparison bounds changed");
            int[] left = Pixels(a), right = Pixels(b);
            int count = 0;
            for (int i = 0; i < left.Length; ++i) {
                for (int shift = 0; shift <= 16; shift += 8) {
                    if (Math.Abs(((left[i] >> shift) & 255) - ((right[i] >> shift) & 255)) > tolerance) { ++count; break; }
                }
            }
            return count;
        }
    }
}
'@
}

function Assert-InputTarget([IntPtr] $Window) {
    $actual = [VelocityDesktop.Native]::Foreground
    if ($actual -ne $Window) {
        throw "Refusing input. Target: $([VelocityDesktop.Native]::Describe($Window)); foreground: $([VelocityDesktop.Native]::Describe($actual))"
    }
}

function Send-Key([IntPtr] $Window, [string[]] $Keys) {
    Assert-InputTarget $Window
    $named = @{ Ctrl=0xa2; Shift=0xa0; Alt=0xa4; Win=0x5b; Esc=27; Enter=13; Backspace=8; Tab=9; Space=32;
        Left=37; Up=38; Right=39; Down=40; Home=36; End=35; PageDown=34; Slash=0xbf }
    [uint32[]] $codes = foreach ($key in $Keys) {
        if ($named.ContainsKey($key)) { $named[$key] }
        elseif ($key -match '^F([1-9]|1[0-2])$') { 111 + [int]$key.Substring(1) }
        elseif ($key -match '^[A-Z0-9]$') { [int][char]$key.ToUpperInvariant() }
        else { throw "Unknown key: $key" }
    }
    [VelocityDesktop.Native]::Chord($codes)
}

function Send-Ascii([IntPtr] $Window, [string] $Value) {
    foreach ($unit in $Value.ToCharArray()) {
        if ($unit -ceq ' ') { Send-Key $Window Space }
        elseif ([char]::IsUpper($unit)) { Send-Key $Window @('Shift', [string]$unit) }
        else { Send-Key $Window ([string]$unit) }
    }
}

function Send-Text([IntPtr] $Window, [string] $Value) {
    Assert-InputTarget $Window
    [VelocityDesktop.Native]::Text($Value)
}

function Get-Elements($Root, [string] $Type, [switch] $Children) {
    $scope = if ($Children) { [TreeScope]::Children } else { [TreeScope]::Descendants }
    $kind = [ControlType].GetField($Type).GetValue($null)
    $filter = [PropertyCondition]::new([AutomationElement]::ControlTypeProperty, $kind)
    $found = $Root.FindAll($scope, $filter)
    for ($i = 0; $i -lt $found.Count; $i++) { $found[$i] }
}

function Get-TopMenus([IntPtr] $Window) {
    $root = [AutomationElement]::FromHandle($Window)
    # A window also exposes its system menu bar; the resource bar has multiple items.
    try {
        foreach ($bar in @(Get-Elements $root MenuBar)) {
            $items = @(Get-Elements $bar MenuItem -Children)
            if ($items.Count -gt 1) { return $items }
        }
    } catch [ElementNotAvailableException], [System.Runtime.InteropServices.COMException] { return }
}

function Get-PopupItems([IntPtr] $Window, [IntPtr] $Popup, [int] $Index) {
    try {
        $root = [AutomationElement]::FromHandle($Popup)
        $items = @(Get-Elements $root MenuItem -Children)
        if (-not $items.Count) { $items = @(Get-Elements $root MenuItem) }
        if ($items.Count) { return $items }
        # The Win32 accessibility proxy can nest the popup under the expanded menu item.
        $top = @(Get-TopMenus $Window)
        if ($Index -lt $top.Count) {
            Get-Elements $top[$Index] MenuItem -Children | Where-Object { -not $_.Current.IsOffscreen }
        }
    } catch [ElementNotAvailableException], [System.Runtime.InteropServices.COMException] { return }
}

function Get-MenuCaption([string] $Label) {
    ((($Label -split "`t", 2)[0] -replace '(?<!&)&(?!&)', '') -replace '&&', '&') -replace '\s*\([A-Z]\)$', ''
}

Export-ModuleMember -Function @('Send-Key', 'Send-Ascii', 'Send-Text',
    'Get-Elements', 'Get-TopMenus', 'Get-PopupItems', 'Get-MenuCaption')
