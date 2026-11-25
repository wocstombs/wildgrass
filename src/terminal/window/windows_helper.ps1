Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class WtMover {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT {
        public int X;
        public int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct WINDOWPLACEMENT {
        public int length;
        public int flags;
        public int showCmd;
        public POINT ptMinPosition;
        public POINT ptMaxPosition;
        public RECT rcNormalPosition;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Auto)]
    public struct MONITORINFO {
        public int cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
    }

    [DllImport("user32.dll")]
    static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int GetClassName(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);

    [DllImport("user32.dll")]
    static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);

    [DllImport("user32.dll")]
    static extern bool IsZoomed(IntPtr hWnd);

    [DllImport("user32.dll")]
    static extern bool IsIconic(IntPtr hWnd);

    [DllImport("user32.dll")]
    static extern bool IsWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    static extern bool GetWindowPlacement(IntPtr hWnd, ref WINDOWPLACEMENT lpwndpl);

    [DllImport("user32.dll")]
    static extern bool SetWindowPlacement(IntPtr hWnd, ref WINDOWPLACEMENT lpwndpl);

    [DllImport("user32.dll")]
    static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint dwFlags);

    [DllImport("user32.dll")]
    static extern IntPtr MonitorFromPoint(POINT pt, uint dwFlags);

    [DllImport("user32.dll", CharSet = CharSet.Auto)]
    static extern bool GetMonitorInfo(IntPtr hMonitor, ref MONITORINFO lpmi);

    [DllImport("user32.dll")]
    static extern bool SetProcessDpiAwarenessContext(IntPtr value);

    [DllImport("user32.dll")]
    static extern uint GetDpiForWindow(IntPtr hwnd);

    [DllImport("dwmapi.dll")]
    static extern int DwmGetWindowAttribute(IntPtr hwnd, int dwAttribute, out RECT pvAttribute, int cbAttribute);

    const uint SWP_NOSIZE = 0x0001;
    const uint SWP_NOMOVE = 0x0002;
    const uint SWP_NOZORDER = 0x0004;
    const uint SWP_NOACTIVATE = 0x0010;
    static readonly IntPtr HWND_TOP = IntPtr.Zero;
    static readonly IntPtr HWND_BOTTOM = new IntPtr(1);
    static readonly IntPtr HWND_TOPMOST = new IntPtr(-1);
    static readonly IntPtr HWND_NOTOPMOST = new IntPtr(-2);
    const uint MONITOR_DEFAULTTONULL = 0;
    const uint MONITOR_DEFAULTTONEAREST = 2;
    const int SW_SHOWNORMAL = 1;
    const int SW_SHOWNOACTIVATE = 4;
    const int SW_RESTORE = 9;
    const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;

    static bool IsTerminalWindow(IntPtr hwnd) {
        if (hwnd == IntPtr.Zero) return false;
        var sb = new StringBuilder(256);
        if (GetClassName(hwnd, sb, sb.Capacity) <= 0) return false;
        string cls = sb.ToString();
        return cls == "CASCADIA_HOSTING_WINDOW_CLASS" || cls == "ConsoleWindowClass";
    }

    static bool SameOrMissingMonitor(IntPtr curMon, int x, int y) {
        IntPtr m = MonitorFromPoint(new POINT { X = x, Y = y }, MONITOR_DEFAULTTONULL);
        return m == IntPtr.Zero || m == curMon;
    }

    static uint WindowDpi(IntPtr hwnd) {
        uint dpi = 96;
        try { dpi = GetDpiForWindow(hwnd); } catch {}
        if (dpi == 0) dpi = 96;
        return dpi;
    }

    // How far the visible frame lies inside the window's rectangle r on each side: Windows counts
    // the invisible borders that resize the window as part of it
    static RECT FrameInsets(IntPtr hwnd, RECT r) {
        RECT inset = new RECT();
        try {
            RECT ext;
            if (DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, out ext, Marshal.SizeOf(typeof(RECT))) == 0) {
                inset.Left = ext.Left - r.Left;
                inset.Top = ext.Top - r.Top;
                inset.Right = r.Right - ext.Right;
                inset.Bottom = r.Bottom - ext.Bottom;
            }
        } catch {}
        return inset;
    }

    static void Move(IntPtr hwnd, int dxSign, int dySign) {
        if (IsZoomed(hwnd)) {
            ShowWindow(hwnd, SW_RESTORE);
        }
        RECT r;
        if (!GetWindowRect(hwnd, out r)) return;

        uint dpi = WindowDpi(hwnd);
        int step = (int)Math.Round(50.0 * dpi / 96.0);
        if (step < 1) step = 1;

        int dx = dxSign * step;
        int dy = dySign * step;
        int w = r.Right - r.Left;
        int h = r.Bottom - r.Top;
        int nx = r.Left + dx;
        int ny = r.Top + dy;

        RECT inset = FrameInsets(hwnd, r);

        IntPtr curMon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        MONITORINFO mi = new MONITORINFO();
        mi.cbSize = Marshal.SizeOf(typeof(MONITORINFO));
        if (curMon != IntPtr.Zero && GetMonitorInfo(curMon, ref mi)) {
            RECT rc = mi.rcWork;
            int curVisLeft = r.Left + inset.Left;
            int curVisTop = r.Top + inset.Top;
            int curVisRight = r.Right - inset.Right;
            int curVisBottom = r.Bottom - inset.Bottom;
            int visLeft = nx + inset.Left;
            int visTop = ny + inset.Top;
            int visRight = nx + w - inset.Right;
            int visBottom = ny + h - inset.Bottom;
            int cy = ny + h / 2;
            int cx = nx + w / 2;

            if (dx < 0 && visLeft < rc.Left && SameOrMissingMonitor(curMon, visLeft, cy)) {
                nx = curVisLeft <= rc.Left ? r.Left : rc.Left - inset.Left;
            } else if (dx > 0 && visRight > rc.Right && SameOrMissingMonitor(curMon, visRight - 1, cy)) {
                nx = curVisRight >= rc.Right ? r.Left : rc.Right - w + inset.Right;
            }
            if (dy < 0 && visTop < rc.Top && SameOrMissingMonitor(curMon, cx, visTop)) {
                ny = curVisTop <= rc.Top ? r.Top : rc.Top - inset.Top;
            } else if (dy > 0 && visBottom > rc.Bottom && SameOrMissingMonitor(curMon, cx, visBottom - 1)) {
                ny = curVisBottom >= rc.Bottom ? r.Top : rc.Bottom - h + inset.Bottom;
            }
        }

        if (nx == r.Left && ny == r.Top) return;
        SetWindowPos(hwnd, IntPtr.Zero, nx, ny, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }

    // Compact window size used when Windows Terminal ignores CSI 8 resize requests (multi-tab
    // windows): visible frame of 929 x 271 px at 192 DPI (47x6 cells in Ubuntu Mono 13 pt),
    // scaled proportionally at other DPIs, keeping the top-left corner fixed.
    const int SizeDpi = 192;
    const int SizeWidth = 929;
    const int SizeHeight = 271;

    static int AtDpi(int pixels, uint dpi) {
        return (int)Math.Round((double)pixels * dpi / SizeDpi);
    }

    // The window "S" resized and how it was before, so that "Q" can give that back as wildgrass
    // exits, wherever the focus is by then
    static IntPtr resized = IntPtr.Zero;
    static WINDOWPLACEMENT resizedFrom;

    static void Resize(IntPtr hwnd) {
        WINDOWPLACEMENT before = new WINDOWPLACEMENT();
        before.length = Marshal.SizeOf(typeof(WINDOWPLACEMENT));
        if (!GetWindowPlacement(hwnd, ref before)) return;
        resized = hwnd;
        resizedFrom = before;
        if (IsZoomed(hwnd) || IsIconic(hwnd)) {
            ShowWindow(hwnd, SW_RESTORE);
        }
        RECT r;
        if (!GetWindowRect(hwnd, out r)) return;
        RECT inset = FrameInsets(hwnd, r);
        uint dpi = WindowDpi(hwnd);
        SetWindowPos(hwnd, IntPtr.Zero, 0, 0,
            AtDpi(SizeWidth, dpi) + inset.Left + inset.Right,
            AtDpi(SizeHeight, dpi) + inset.Top + inset.Bottom,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }

    // Gives the window "S" resized back the size, place and maximized state it had before
    static void UndoResize() {
        if (resized != IntPtr.Zero && IsWindow(resized)) {
            WINDOWPLACEMENT wp = resizedFrom;
            // Shown as it was, without bringing it forward
            if (wp.showCmd == SW_SHOWNORMAL) wp.showCmd = SW_SHOWNOACTIVATE;
            SetWindowPlacement(resized, ref wp);
        }
        resized = IntPtr.Zero;
    }

    // The window last put above or below the others, so that it can be put back wherever the
    // focus is by then (wildgrass asks for that as it exits)
    static IntPtr layered = IntPtr.Zero;

    // "T" above every other window, "B" below every other window, anything else back to the
    // usual stacking. Never activates the window, so it is only a change in what covers what.
    public static void SetLayer(IntPtr hwnd, string mode) {
        uint flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        if (mode == "T") {
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        } else if (mode == "B") {
            // A topmost window sent to the bottom stops being topmost
            SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, flags);
        } else {
            // Leaves the topmost group. From the bottom that does nothing, so the window is
            // also raised among the ordinary ones.
            SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
            SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, flags);
        }
    }

    // Only a window this helper layered is put back, wherever the focus is by now: with
    // nothing layered there is nothing to put back, and the foreground window may be another
    // terminal that someone else pinned
    static void PutBackLayer() {
        if (layered != IntPtr.Zero && IsWindow(layered)) SetLayer(layered, "N");
        layered = IntPtr.Zero;
    }

    // The commands for the foreground window, when it is a terminal's
    static void OnForegroundWindow(string command) {
        IntPtr hwnd = GetForegroundWindow();
        if (!IsTerminalWindow(hwnd)) return;
        switch (command) {
            case "S": Resize(hwnd); break;
            case "T":
            case "B":
                if (layered != IntPtr.Zero && layered != hwnd && IsWindow(layered)) {
                    SetLayer(layered, "N");
                }
                SetLayer(hwnd, command);
                layered = hwnd;
                break;
            case "L": Move(hwnd, -1, 0); break;
            case "R": Move(hwnd, 1, 0); break;
            case "U": Move(hwnd, 0, -1); break;
            case "D": Move(hwnd, 0, 1); break;
        }
    }

    public static void Run() {
        try { SetProcessDpiAwarenessContext(new IntPtr(-4)); } catch {}
        string line;
        while ((line = Console.In.ReadLine()) != null) {
            string command = line.Trim();
            switch (command) {
                // These act on the window this helper changed earlier, not the foreground one
                case "N": PutBackLayer(); break;
                case "Q": UndoResize(); break;
                default: OnForegroundWindow(command); break;
            }
        }
    }
}
'@
[WtMover]::Run()
