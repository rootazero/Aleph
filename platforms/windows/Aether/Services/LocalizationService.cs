using Windows.ApplicationModel.Resources;

namespace Aether.Services;

/// <summary>
/// Localization service for accessing localized strings.
///
/// Uses Windows Resource Manager (.resw files) for localization.
/// Supports:
/// - English (en-US)
/// - Simplified Chinese (zh-Hans)
///
/// Usage:
///   var text = LocalizationService.GetString("Settings_General");
///   var text = L("Settings_General"); // Using static import
/// </summary>
public static class LocalizationService
{
    private static readonly ResourceLoader _resourceLoader;

    static LocalizationService()
    {
        _resourceLoader = new ResourceLoader();
    }

    /// <summary>
    /// Get a localized string by key.
    /// </summary>
    /// <param name="key">The resource key (e.g., "Settings_General")</param>
    /// <returns>The localized string, or the key if not found</returns>
    public static string GetString(string key)
    {
        try
        {
            var value = _resourceLoader.GetString(key);
            return string.IsNullOrEmpty(value) ? key : value;
        }
        catch
        {
            return key;
        }
    }

    /// <summary>
    /// Get a localized string with format arguments.
    /// </summary>
    /// <param name="key">The resource key</param>
    /// <param name="args">Format arguments</param>
    /// <returns>The formatted localized string</returns>
    public static string GetString(string key, params object[] args)
    {
        var format = GetString(key);
        try
        {
            return string.Format(format, args);
        }
        catch
        {
            return format;
        }
    }

    /// <summary>
    /// Short alias for GetString (for convenience).
    /// </summary>
    public static string L(string key) => GetString(key);

    /// <summary>
    /// Short alias for GetString with format arguments.
    /// </summary>
    public static string L(string key, params object[] args) => GetString(key, args);

    /// <summary>
    /// Get the current language code.
    /// </summary>
    public static string CurrentLanguage
    {
        get
        {
            var languages = Windows.Globalization.ApplicationLanguages.Languages;
            return languages.Count > 0 ? languages[0] : "en-US";
        }
    }

    /// <summary>
    /// Check if the current language is Chinese.
    /// </summary>
    public static bool IsChinese => CurrentLanguage.StartsWith("zh", StringComparison.OrdinalIgnoreCase);

    /// <summary>
    /// Check if the current language is English.
    /// </summary>
    public static bool IsEnglish => CurrentLanguage.StartsWith("en", StringComparison.OrdinalIgnoreCase);
}

/// <summary>
/// Static imports helper for easy localization access.
/// Add: using static Aether.Services.Localization;
/// Then use: L("key") directly in code.
/// </summary>
public static class Localization
{
    /// <summary>
    /// Get a localized string by key.
    /// </summary>
    public static string L(string key) => LocalizationService.GetString(key);

    /// <summary>
    /// Get a localized string with format arguments.
    /// </summary>
    public static string L(string key, params object[] args) => LocalizationService.GetString(key, args);
}
