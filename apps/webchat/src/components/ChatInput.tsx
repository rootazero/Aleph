import React, { useState, useRef, useEffect, useCallback } from 'react'

interface CommandInfo {
  key: string
  description: string
  icon: string
  hint?: string
  command_type: string
  source_type: string
}

interface ChatInputProps {
  onSend: (message: string) => void
  disabled?: boolean
  placeholder?: string
  fetchCommands?: () => Promise<CommandInfo[]>
}

export function ChatInput({ onSend, disabled = false, placeholder = 'Type a message...', fetchCommands }: ChatInputProps) {
  const [input, setInput] = useState('')
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  // Slash command state
  const [showPalette, setShowPalette] = useState(false)
  const [commands, setCommands] = useState<CommandInfo[]>([])
  const [filtered, setFiltered] = useState<CommandInfo[]>([])
  const [selectedIndex, setSelectedIndex] = useState(0)
  const [commandsLoaded, setCommandsLoaded] = useState(false)
  const paletteRef = useRef<HTMLDivElement>(null)

  // Auto-resize textarea
  useEffect(() => {
    const textarea = textareaRef.current
    if (textarea) {
      textarea.style.height = 'auto'
      textarea.style.height = `${Math.min(textarea.scrollHeight, 200)}px`
    }
  }, [input])

  // Load commands on first / keystroke
  const ensureCommands = useCallback(async () => {
    if (commandsLoaded || !fetchCommands) return
    try {
      const cmds = await fetchCommands()
      setCommands(cmds)
      setCommandsLoaded(true)
      return cmds
    } catch (e) {
      console.error('Failed to fetch commands:', e)
      return []
    }
  }, [fetchCommands, commandsLoaded])

  // Filter commands based on current input
  const updateFiltered = useCallback((value: string, cmds: CommandInfo[]) => {
    if (!value.startsWith('/')) {
      setShowPalette(false)
      return
    }

    const prefix = value.slice(1).toLowerCase()
    const matches = prefix
      ? cmds.filter(c => c.key.toLowerCase().startsWith(prefix))
      : cmds

    setFiltered(matches)
    setSelectedIndex(0)
    setShowPalette(matches.length > 0)
  }, [])

  // Handle input change
  const handleChange = useCallback(async (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const value = e.target.value
    setInput(value)

    if (value.startsWith('/') && fetchCommands) {
      let cmds = commands
      if (!commandsLoaded) {
        cmds = (await ensureCommands()) || []
      }
      updateFiltered(value, cmds)
    } else {
      setShowPalette(false)
    }
  }, [commands, commandsLoaded, ensureCommands, fetchCommands, updateFiltered])

  // Select a command from palette
  const selectCommand = useCallback((key: string) => {
    setInput(`/${key} `)
    setShowPalette(false)
    textareaRef.current?.focus()
  }, [])

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (input.trim() && !disabled) {
      onSend(input.trim())
      setInput('')
      setShowPalette(false)
    }
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (showPalette && filtered.length > 0) {
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault()
          setSelectedIndex(prev => (prev + 1) % filtered.length)
          return
        case 'ArrowUp':
          e.preventDefault()
          setSelectedIndex(prev => (prev === 0 ? filtered.length - 1 : prev - 1))
          return
        case 'Tab':
        case 'Enter':
          e.preventDefault()
          selectCommand(filtered[selectedIndex].key)
          return
        case 'Escape':
          e.preventDefault()
          setShowPalette(false)
          return
      }
    }

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSubmit(e)
    }
  }

  // Scroll selected item into view
  useEffect(() => {
    if (showPalette && paletteRef.current) {
      const selectedEl = paletteRef.current.children[selectedIndex] as HTMLElement
      selectedEl?.scrollIntoView({ block: 'nearest' })
    }
  }, [selectedIndex, showPalette])

  return (
    <form onSubmit={handleSubmit} className="relative flex items-end gap-2 p-4 border-t border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800">
      {/* Command palette dropdown */}
      {showPalette && filtered.length > 0 && (
        <div
          ref={paletteRef}
          className="absolute bottom-full left-4 right-16 mb-2 max-h-72 overflow-y-auto
                     bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-600
                     rounded-xl shadow-xl z-50"
        >
          {filtered.map((cmd, i) => (
            <button
              key={cmd.key}
              type="button"
              className={`w-full flex items-center gap-3 px-4 py-2.5 text-left transition-colors
                ${i === selectedIndex
                  ? 'bg-aleph-50 dark:bg-aleph-900/30 text-aleph-700 dark:text-aleph-300'
                  : 'hover:bg-gray-50 dark:hover:bg-gray-800 text-gray-700 dark:text-gray-300'
                }
                ${i === 0 ? 'rounded-t-xl' : ''}
                ${i === filtered.length - 1 ? 'rounded-b-xl' : ''}
              `}
              onMouseDown={(e) => {
                e.preventDefault()
                selectCommand(cmd.key)
              }}
              onMouseEnter={() => setSelectedIndex(i)}
            >
              <span className="shrink-0 text-xs font-mono font-semibold text-aleph-600 dark:text-aleph-400 bg-aleph-100 dark:bg-aleph-900/50 px-2 py-0.5 rounded">
                /{cmd.key}
              </span>
              <span className="text-sm truncate">{cmd.description}</span>
              <span className="ml-auto text-xs text-gray-400 dark:text-gray-500 shrink-0">{cmd.source_type}</span>
            </button>
          ))}
        </div>
      )}

      <div className="flex-1 relative">
        <textarea
          ref={textareaRef}
          value={input}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          disabled={disabled}
          rows={1}
          className="w-full resize-none rounded-xl border border-gray-300 dark:border-gray-600
                     bg-gray-50 dark:bg-gray-900 px-4 py-3 pr-12
                     text-gray-900 dark:text-white placeholder-gray-500
                     focus:outline-none focus:ring-2 focus:ring-aleph-500 focus:border-transparent
                     disabled:opacity-50 disabled:cursor-not-allowed
                     transition-colors"
        />
      </div>
      <button
        type="submit"
        disabled={disabled || !input.trim()}
        className="flex items-center justify-center w-12 h-12 rounded-xl
                   bg-aleph-600 hover:bg-aleph-700 disabled:bg-gray-300 dark:disabled:bg-gray-600
                   text-white disabled:text-gray-500
                   transition-colors disabled:cursor-not-allowed"
        aria-label="Send message"
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 24 24"
          fill="currentColor"
          className="w-5 h-5"
        >
          <path d="M3.478 2.405a.75.75 0 00-.926.94l2.432 7.905H13.5a.75.75 0 010 1.5H4.984l-2.432 7.905a.75.75 0 00.926.94 60.519 60.519 0 0018.445-8.986.75.75 0 000-1.218A60.517 60.517 0 003.478 2.405z" />
        </svg>
      </button>
    </form>
  )
}
