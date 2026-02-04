namespace Aleph.ViewModels;

/// <summary>
/// Represents the current content display state of the conversation window.
/// </summary>
public enum ContentDisplayState
{
    /// <summary>
    /// No messages, no prefix - show input only.
    /// </summary>
    Empty,

    /// <summary>
    /// Has messages - show messages + input.
    /// </summary>
    Conversation,

    /// <summary>
    /// "/" prefix - show command list.
    /// </summary>
    CommandList,

    /// <summary>
    /// "//" prefix - show topic list.
    /// </summary>
    TopicList
}
