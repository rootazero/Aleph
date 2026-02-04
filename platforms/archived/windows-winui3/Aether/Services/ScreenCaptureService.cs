using System.Runtime.InteropServices;
using Windows.Graphics.Imaging;
using Windows.Storage.Streams;

namespace Aleph.Services;

/// <summary>
/// Screen capture service for capturing screen content.
///
/// Features:
/// - Capture full screen
/// - Capture specific region
/// - Capture active window
/// - Save as PNG/JPEG bytes
///
/// Used for:
/// - Vision capabilities (image understanding)
/// - Screen context for AI assistance
/// </summary>
public sealed class ScreenCaptureService : IDisposable
{
    #region Win32 Structures and Imports

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    private static extern IntPtr GetDesktopWindow();

    [DllImport("user32.dll")]
    private static extern IntPtr GetWindowDC(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern int ReleaseDC(IntPtr hWnd, IntPtr hDC);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    private static extern IntPtr GetForegroundWindow();

    [DllImport("gdi32.dll")]
    private static extern IntPtr CreateCompatibleDC(IntPtr hdc);

    [DllImport("gdi32.dll")]
    private static extern IntPtr CreateCompatibleBitmap(IntPtr hdc, int nWidth, int nHeight);

    [DllImport("gdi32.dll")]
    private static extern IntPtr SelectObject(IntPtr hdc, IntPtr hgdiobj);

    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool BitBlt(IntPtr hdcDest, int nXDest, int nYDest, int nWidth, int nHeight,
        IntPtr hdcSrc, int nXSrc, int nYSrc, uint dwRop);

    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteObject(IntPtr hObject);

    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteDC(IntPtr hdc);

    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int nIndex);

    private const int SM_CXSCREEN = 0;
    private const int SM_CYSCREEN = 1;
    private const int SM_XVIRTUALSCREEN = 76;
    private const int SM_YVIRTUALSCREEN = 77;
    private const int SM_CXVIRTUALSCREEN = 78;
    private const int SM_CYVIRTUALSCREEN = 79;

    private const uint SRCCOPY = 0x00CC0020;

    #endregion

    private bool _disposed;

    /// <summary>
    /// Capture the entire primary screen.
    /// </summary>
    /// <returns>PNG image bytes, or null on failure</returns>
    public async Task<byte[]?> CaptureScreenAsync()
    {
        int width = GetSystemMetrics(SM_CXSCREEN);
        int height = GetSystemMetrics(SM_CYSCREEN);

        return await CaptureRegionAsync(0, 0, width, height);
    }

    /// <summary>
    /// Capture all screens (virtual screen).
    /// </summary>
    /// <returns>PNG image bytes, or null on failure</returns>
    public async Task<byte[]?> CaptureAllScreensAsync()
    {
        int x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        int y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        int width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        int height = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        return await CaptureRegionAsync(x, y, width, height);
    }

    /// <summary>
    /// Capture the currently active (foreground) window.
    /// </summary>
    /// <returns>PNG image bytes, or null on failure</returns>
    public async Task<byte[]?> CaptureActiveWindowAsync()
    {
        var hwnd = GetForegroundWindow();
        if (hwnd == IntPtr.Zero)
            return null;

        if (!GetWindowRect(hwnd, out RECT rect))
            return null;

        int width = rect.Right - rect.Left;
        int height = rect.Bottom - rect.Top;

        if (width <= 0 || height <= 0)
            return null;

        return await CaptureRegionAsync(rect.Left, rect.Top, width, height);
    }

    /// <summary>
    /// Capture a specific region of the screen.
    /// </summary>
    /// <param name="x">Left coordinate</param>
    /// <param name="y">Top coordinate</param>
    /// <param name="width">Width in pixels</param>
    /// <param name="height">Height in pixels</param>
    /// <returns>PNG image bytes, or null on failure</returns>
    public async Task<byte[]?> CaptureRegionAsync(int x, int y, int width, int height)
    {
        if (width <= 0 || height <= 0)
            return null;

        IntPtr hdcScreen = IntPtr.Zero;
        IntPtr hdcMem = IntPtr.Zero;
        IntPtr hBitmap = IntPtr.Zero;
        IntPtr hOldBitmap = IntPtr.Zero;

        try
        {
            // Get screen DC
            var desktopHwnd = GetDesktopWindow();
            hdcScreen = GetWindowDC(desktopHwnd);
            if (hdcScreen == IntPtr.Zero)
                return null;

            // Create compatible DC and bitmap
            hdcMem = CreateCompatibleDC(hdcScreen);
            if (hdcMem == IntPtr.Zero)
                return null;

            hBitmap = CreateCompatibleBitmap(hdcScreen, width, height);
            if (hBitmap == IntPtr.Zero)
                return null;

            hOldBitmap = SelectObject(hdcMem, hBitmap);

            // Copy screen content to bitmap
            if (!BitBlt(hdcMem, 0, 0, width, height, hdcScreen, x, y, SRCCOPY))
                return null;

            SelectObject(hdcMem, hOldBitmap);

            // Convert to PNG bytes using System.Drawing
            using var bitmap = System.Drawing.Image.FromHbitmap(hBitmap);
            using var stream = new MemoryStream();
            bitmap.Save(stream, System.Drawing.Imaging.ImageFormat.Png);
            return stream.ToArray();
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[ScreenCapture] Error: {ex.Message}");
            return null;
        }
        finally
        {
            if (hBitmap != IntPtr.Zero) DeleteObject(hBitmap);
            if (hdcMem != IntPtr.Zero) DeleteDC(hdcMem);
            if (hdcScreen != IntPtr.Zero) ReleaseDC(GetDesktopWindow(), hdcScreen);
        }
    }

    /// <summary>
    /// Capture screen around the cursor position.
    /// </summary>
    /// <param name="cursorService">Cursor service for position</param>
    /// <param name="radius">Capture radius around cursor</param>
    /// <returns>PNG image bytes, or null on failure</returns>
    public async Task<byte[]?> CaptureAroundCursorAsync(CursorService cursorService, int radius = 200)
    {
        var (cursorX, cursorY) = cursorService.GetCursorPosition();

        int x = cursorX - radius;
        int y = cursorY - radius;
        int size = radius * 2;

        // Ensure we don't go negative
        if (x < 0) x = 0;
        if (y < 0) y = 0;

        return await CaptureRegionAsync(x, y, size, size);
    }

    /// <summary>
    /// Get screen dimensions.
    /// </summary>
    public (int Width, int Height) GetPrimaryScreenSize()
    {
        return (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    }

    /// <summary>
    /// Get virtual screen dimensions (all monitors combined).
    /// </summary>
    public (int X, int Y, int Width, int Height) GetVirtualScreenBounds()
    {
        return (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN)
        );
    }

    /// <summary>
    /// Convert PNG bytes to JPEG bytes with specified quality.
    /// </summary>
    /// <param name="pngBytes">PNG image bytes</param>
    /// <param name="quality">JPEG quality (0-100)</param>
    /// <returns>JPEG image bytes</returns>
    public byte[]? ConvertToJpeg(byte[] pngBytes, int quality = 85)
    {
        try
        {
            using var inputStream = new MemoryStream(pngBytes);
            using var bitmap = System.Drawing.Image.FromStream(inputStream);
            using var outputStream = new MemoryStream();

            var encoder = System.Drawing.Imaging.ImageCodecInfo.GetImageEncoders()
                .First(c => c.FormatID == System.Drawing.Imaging.ImageFormat.Jpeg.Guid);

            var encoderParams = new System.Drawing.Imaging.EncoderParameters(1);
            encoderParams.Param[0] = new System.Drawing.Imaging.EncoderParameter(
                System.Drawing.Imaging.Encoder.Quality, quality);

            bitmap.Save(outputStream, encoder, encoderParams);
            return outputStream.ToArray();
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// Resize image bytes to specified dimensions.
    /// </summary>
    /// <param name="imageBytes">Image bytes (PNG or JPEG)</param>
    /// <param name="maxWidth">Maximum width</param>
    /// <param name="maxHeight">Maximum height</param>
    /// <returns>Resized PNG image bytes</returns>
    public byte[]? ResizeImage(byte[] imageBytes, int maxWidth, int maxHeight)
    {
        try
        {
            using var inputStream = new MemoryStream(imageBytes);
            using var original = System.Drawing.Image.FromStream(inputStream);

            // Calculate new dimensions
            double ratioX = (double)maxWidth / original.Width;
            double ratioY = (double)maxHeight / original.Height;
            double ratio = Math.Min(ratioX, ratioY);

            if (ratio >= 1) return imageBytes; // No need to resize

            int newWidth = (int)(original.Width * ratio);
            int newHeight = (int)(original.Height * ratio);

            using var resized = new System.Drawing.Bitmap(newWidth, newHeight);
            using var graphics = System.Drawing.Graphics.FromImage(resized);

            graphics.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
            graphics.DrawImage(original, 0, 0, newWidth, newHeight);

            using var outputStream = new MemoryStream();
            resized.Save(outputStream, System.Drawing.Imaging.ImageFormat.Png);
            return outputStream.ToArray();
        }
        catch
        {
            return null;
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        GC.SuppressFinalize(this);
    }
}
