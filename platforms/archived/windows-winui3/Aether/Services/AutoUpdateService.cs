using System.Net.Http;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Aleph.Services;

/// <summary>
/// Automatic update service for checking and downloading updates.
///
/// Features:
/// - Check for updates from GitHub releases
/// - Download update package
/// - Apply updates with restart
///
/// Update flow:
/// 1. Check GitHub API for latest release
/// 2. Compare versions
/// 3. Download if newer version available
/// 4. Prompt user to restart and apply
/// </summary>
public sealed class AutoUpdateService : IDisposable
{
    private readonly HttpClient _httpClient;
    private readonly string _currentVersion;
    private readonly string _repoOwner;
    private readonly string _repoName;
    private bool _disposed;

    /// <summary>
    /// Event raised when a new version is available.
    /// </summary>
    public event Action<UpdateInfo>? UpdateAvailable;

    /// <summary>
    /// Event raised when download progress changes.
    /// </summary>
    public event Action<double>? DownloadProgress;

    /// <summary>
    /// Event raised when download completes.
    /// </summary>
    public event Action<string>? DownloadComplete;

    /// <summary>
    /// Event raised on error.
    /// </summary>
    public event Action<string>? Error;

    public AutoUpdateService(string repoOwner = "anthropics", string repoName = "aleph")
    {
        _repoOwner = repoOwner;
        _repoName = repoName;

        _httpClient = new HttpClient();
        _httpClient.DefaultRequestHeaders.Add("User-Agent", "Aleph-AutoUpdate");
        _httpClient.DefaultRequestHeaders.Add("Accept", "application/vnd.github.v3+json");

        // Get current version from assembly
        var version = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
        _currentVersion = version?.ToString(3) ?? "0.1.0";
    }

    /// <summary>
    /// Check for updates from GitHub releases.
    /// </summary>
    /// <returns>Update info if available, null otherwise</returns>
    public async Task<UpdateInfo?> CheckForUpdatesAsync()
    {
        try
        {
            var url = $"https://api.github.com/repos/{_repoOwner}/{_repoName}/releases/latest";
            var response = await _httpClient.GetStringAsync(url);

            var release = JsonSerializer.Deserialize<GitHubRelease>(response);
            if (release == null)
                return null;

            var latestVersion = release.TagName?.TrimStart('v') ?? "";
            if (string.IsNullOrEmpty(latestVersion))
                return null;

            // Compare versions
            if (!IsNewerVersion(latestVersion, _currentVersion))
            {
                System.Diagnostics.Debug.WriteLine($"[AutoUpdate] Current version {_currentVersion} is up to date");
                return null;
            }

            // Find Windows asset
            var windowsAsset = release.Assets?.FirstOrDefault(a =>
                a.Name?.Contains("windows", StringComparison.OrdinalIgnoreCase) == true &&
                (a.Name.EndsWith(".msix") || a.Name.EndsWith(".exe") || a.Name.EndsWith(".zip")));

            var updateInfo = new UpdateInfo
            {
                Version = latestVersion,
                CurrentVersion = _currentVersion,
                ReleaseNotes = release.Body ?? "",
                PublishedAt = release.PublishedAt,
                DownloadUrl = windowsAsset?.BrowserDownloadUrl ?? release.HtmlUrl ?? "",
                FileName = windowsAsset?.Name ?? $"aleph-{latestVersion}-windows.zip",
                FileSize = windowsAsset?.Size ?? 0,
                ReleaseUrl = release.HtmlUrl ?? ""
            };

            UpdateAvailable?.Invoke(updateInfo);
            return updateInfo;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[AutoUpdate] Check failed: {ex.Message}");
            Error?.Invoke($"Failed to check for updates: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Download the update package.
    /// </summary>
    /// <param name="updateInfo">Update info from CheckForUpdatesAsync</param>
    /// <returns>Path to downloaded file, or null on failure</returns>
    public async Task<string?> DownloadUpdateAsync(UpdateInfo updateInfo)
    {
        if (string.IsNullOrEmpty(updateInfo.DownloadUrl))
        {
            Error?.Invoke("No download URL available");
            return null;
        }

        try
        {
            var downloadPath = Path.Combine(
                Path.GetTempPath(),
                "AlephUpdates",
                updateInfo.FileName
            );

            Directory.CreateDirectory(Path.GetDirectoryName(downloadPath)!);

            using var response = await _httpClient.GetAsync(updateInfo.DownloadUrl, HttpCompletionOption.ResponseHeadersRead);
            response.EnsureSuccessStatusCode();

            var totalBytes = response.Content.Headers.ContentLength ?? updateInfo.FileSize;

            using var contentStream = await response.Content.ReadAsStreamAsync();
            using var fileStream = new FileStream(downloadPath, FileMode.Create, FileAccess.Write, FileShare.None);

            var buffer = new byte[8192];
            long totalRead = 0;
            int bytesRead;

            while ((bytesRead = await contentStream.ReadAsync(buffer)) > 0)
            {
                await fileStream.WriteAsync(buffer.AsMemory(0, bytesRead));
                totalRead += bytesRead;

                if (totalBytes > 0)
                {
                    var progress = (double)totalRead / totalBytes * 100;
                    DownloadProgress?.Invoke(progress);
                }
            }

            DownloadComplete?.Invoke(downloadPath);
            return downloadPath;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[AutoUpdate] Download failed: {ex.Message}");
            Error?.Invoke($"Failed to download update: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Apply the downloaded update.
    /// This will restart the application.
    /// </summary>
    /// <param name="updateFilePath">Path to downloaded update file</param>
    public void ApplyUpdate(string updateFilePath)
    {
        if (!File.Exists(updateFilePath))
        {
            Error?.Invoke("Update file not found");
            return;
        }

        try
        {
            // For MSIX packages
            if (updateFilePath.EndsWith(".msix", StringComparison.OrdinalIgnoreCase))
            {
                // Open MSIX with default handler (App Installer)
                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo
                {
                    FileName = updateFilePath,
                    UseShellExecute = true
                });
            }
            // For EXE installer
            else if (updateFilePath.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
            {
                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo
                {
                    FileName = updateFilePath,
                    UseShellExecute = true
                });
            }
            // For ZIP archive
            else if (updateFilePath.EndsWith(".zip", StringComparison.OrdinalIgnoreCase))
            {
                // Open containing folder
                System.Diagnostics.Process.Start("explorer.exe", $"/select,\"{updateFilePath}\"");
            }
        }
        catch (Exception ex)
        {
            Error?.Invoke($"Failed to apply update: {ex.Message}");
        }
    }

    /// <summary>
    /// Compare two version strings.
    /// </summary>
    private static bool IsNewerVersion(string newVersion, string currentVersion)
    {
        try
        {
            var newVer = Version.Parse(newVersion.Split('-')[0]); // Handle pre-release tags
            var curVer = Version.Parse(currentVersion.Split('-')[0]);
            return newVer > curVer;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>
    /// Clean up old update files.
    /// </summary>
    public void CleanupOldUpdates()
    {
        try
        {
            var updateDir = Path.Combine(Path.GetTempPath(), "AlephUpdates");
            if (Directory.Exists(updateDir))
            {
                foreach (var file in Directory.GetFiles(updateDir))
                {
                    try
                    {
                        var fileInfo = new FileInfo(file);
                        if (fileInfo.LastWriteTime < DateTime.Now.AddDays(-7))
                        {
                            File.Delete(file);
                        }
                    }
                    catch { }
                }
            }
        }
        catch { }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _httpClient.Dispose();
        GC.SuppressFinalize(this);
    }
}

/// <summary>
/// Information about an available update.
/// </summary>
public class UpdateInfo
{
    public string Version { get; set; } = "";
    public string CurrentVersion { get; set; } = "";
    public string ReleaseNotes { get; set; } = "";
    public DateTime? PublishedAt { get; set; }
    public string DownloadUrl { get; set; } = "";
    public string FileName { get; set; } = "";
    public long FileSize { get; set; }
    public string ReleaseUrl { get; set; } = "";

    public string FileSizeFormatted => FileSize switch
    {
        < 1024 => $"{FileSize} B",
        < 1024 * 1024 => $"{FileSize / 1024.0:F1} KB",
        _ => $"{FileSize / (1024.0 * 1024.0):F1} MB"
    };
}

/// <summary>
/// GitHub release API response.
/// </summary>
internal class GitHubRelease
{
    [JsonPropertyName("tag_name")]
    public string? TagName { get; set; }

    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("body")]
    public string? Body { get; set; }

    [JsonPropertyName("html_url")]
    public string? HtmlUrl { get; set; }

    [JsonPropertyName("published_at")]
    public DateTime? PublishedAt { get; set; }

    [JsonPropertyName("assets")]
    public List<GitHubAsset>? Assets { get; set; }
}

/// <summary>
/// GitHub release asset.
/// </summary>
internal class GitHubAsset
{
    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("browser_download_url")]
    public string? BrowserDownloadUrl { get; set; }

    [JsonPropertyName("size")]
    public long Size { get; set; }
}
