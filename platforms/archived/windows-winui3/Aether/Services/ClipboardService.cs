using System.Runtime.InteropServices;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage.Streams;

namespace Aether.Services;

/// <summary>
/// Clipboard service for reading and writing various content types.
///
/// Supports:
/// - Plain text
/// - Rich text (RTF)
/// - HTML
/// - Images (PNG, JPEG)
///
/// Uses WinRT Clipboard API for modern clipboard access.
/// </summary>
public sealed class ClipboardService
{
    #region Read Operations

    /// <summary>
    /// Get text content from clipboard.
    /// </summary>
    public async Task<string?> GetTextAsync()
    {
        try
        {
            var content = Clipboard.GetContent();
            if (content.Contains(StandardDataFormats.Text))
            {
                return await content.GetTextAsync();
            }
            return null;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] GetText error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Get HTML content from clipboard.
    /// </summary>
    public async Task<string?> GetHtmlAsync()
    {
        try
        {
            var content = Clipboard.GetContent();
            if (content.Contains(StandardDataFormats.Html))
            {
                return await content.GetHtmlFormatAsync();
            }
            return null;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] GetHtml error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Get RTF content from clipboard.
    /// </summary>
    public async Task<string?> GetRtfAsync()
    {
        try
        {
            var content = Clipboard.GetContent();
            if (content.Contains(StandardDataFormats.Rtf))
            {
                return await content.GetRtfAsync();
            }
            return null;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] GetRtf error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Get image from clipboard as byte array.
    /// </summary>
    public async Task<byte[]?> GetImageBytesAsync()
    {
        try
        {
            var content = Clipboard.GetContent();
            if (content.Contains(StandardDataFormats.Bitmap))
            {
                var streamRef = await content.GetBitmapAsync();
                using var stream = await streamRef.OpenReadAsync();
                using var reader = new DataReader(stream);
                await reader.LoadAsync((uint)stream.Size);
                var bytes = new byte[stream.Size];
                reader.ReadBytes(bytes);
                return bytes;
            }
            return null;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] GetImage error: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Check what content types are available.
    /// </summary>
    public ClipboardContentType GetAvailableContentTypes()
    {
        var types = ClipboardContentType.None;

        try
        {
            var content = Clipboard.GetContent();

            if (content.Contains(StandardDataFormats.Text))
                types |= ClipboardContentType.Text;

            if (content.Contains(StandardDataFormats.Html))
                types |= ClipboardContentType.Html;

            if (content.Contains(StandardDataFormats.Rtf))
                types |= ClipboardContentType.Rtf;

            if (content.Contains(StandardDataFormats.Bitmap))
                types |= ClipboardContentType.Image;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] GetContentTypes error: {ex.Message}");
        }

        return types;
    }

    #endregion

    #region Write Operations

    /// <summary>
    /// Set text content to clipboard.
    /// </summary>
    public void SetText(string text)
    {
        try
        {
            var package = new DataPackage();
            package.SetText(text);
            Clipboard.SetContent(package);
            Clipboard.Flush(); // Persist after app closes
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] SetText error: {ex.Message}");
        }
    }

    /// <summary>
    /// Set HTML content to clipboard (also sets plain text fallback).
    /// </summary>
    public void SetHtml(string html, string? plainText = null)
    {
        try
        {
            var package = new DataPackage();
            package.SetHtmlFormat(html);

            if (!string.IsNullOrEmpty(plainText))
            {
                package.SetText(plainText);
            }

            Clipboard.SetContent(package);
            Clipboard.Flush();
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] SetHtml error: {ex.Message}");
        }
    }

    /// <summary>
    /// Set RTF content to clipboard (also sets plain text fallback).
    /// </summary>
    public void SetRtf(string rtf, string? plainText = null)
    {
        try
        {
            var package = new DataPackage();
            package.SetRtf(rtf);

            if (!string.IsNullOrEmpty(plainText))
            {
                package.SetText(plainText);
            }

            Clipboard.SetContent(package);
            Clipboard.Flush();
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] SetRtf error: {ex.Message}");
        }
    }

    /// <summary>
    /// Clear clipboard content.
    /// </summary>
    public void Clear()
    {
        try
        {
            Clipboard.Clear();
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Clipboard] Clear error: {ex.Message}");
        }
    }

    #endregion

    #region Clipboard Operations (Cut/Copy/Paste simulation)

    /// <summary>
    /// Simulate Cut (Ctrl+X) and return the clipboard content.
    /// </summary>
    public async Task<ClipboardContent?> SimulateCutAsync()
    {
        // Save current clipboard
        var savedText = await GetTextAsync();

        // Simulate Ctrl+X
        KeyboardSimulator.SendKeyCombo(VirtualKey.Control, VirtualKey.X);

        // Wait for clipboard to update
        await Task.Delay(100);

        // Get new content
        var content = await GetClipboardContentAsync();

        return content;
    }

    /// <summary>
    /// Simulate Copy (Ctrl+C) and return the clipboard content.
    /// </summary>
    public async Task<ClipboardContent?> SimulateCopyAsync()
    {
        // Simulate Ctrl+C
        KeyboardSimulator.SendKeyCombo(VirtualKey.Control, VirtualKey.C);

        // Wait for clipboard to update
        await Task.Delay(100);

        // Get content
        return await GetClipboardContentAsync();
    }

    /// <summary>
    /// Simulate Paste (Ctrl+V).
    /// </summary>
    public void SimulatePaste()
    {
        KeyboardSimulator.SendKeyCombo(VirtualKey.Control, VirtualKey.V);
    }

    /// <summary>
    /// Get all available clipboard content.
    /// </summary>
    public async Task<ClipboardContent?> GetClipboardContentAsync()
    {
        var types = GetAvailableContentTypes();
        if (types == ClipboardContentType.None)
            return null;

        return new ClipboardContent
        {
            ContentTypes = types,
            Text = types.HasFlag(ClipboardContentType.Text) ? await GetTextAsync() : null,
            Html = types.HasFlag(ClipboardContentType.Html) ? await GetHtmlAsync() : null,
            Rtf = types.HasFlag(ClipboardContentType.Rtf) ? await GetRtfAsync() : null,
            ImageBytes = types.HasFlag(ClipboardContentType.Image) ? await GetImageBytesAsync() : null,
        };
    }

    #endregion
}

/// <summary>
/// Available clipboard content types.
/// </summary>
[Flags]
public enum ClipboardContentType
{
    None = 0,
    Text = 1,
    Html = 2,
    Rtf = 4,
    Image = 8
}

/// <summary>
/// Clipboard content container.
/// </summary>
public class ClipboardContent
{
    public ClipboardContentType ContentTypes { get; init; }
    public string? Text { get; init; }
    public string? Html { get; init; }
    public string? Rtf { get; init; }
    public byte[]? ImageBytes { get; init; }

    public bool HasText => !string.IsNullOrEmpty(Text);
    public bool HasImage => ImageBytes != null && ImageBytes.Length > 0;
}

// VirtualKey enum is defined in KeyboardSimulator.cs
