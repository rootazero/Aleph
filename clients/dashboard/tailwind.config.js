/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./index.html",
    "./src/**/*.rs",
  ],
  theme: {
    extend: {
      colors: {
        'agent': {
          'thinking': '#3b82f6',
          'acting': '#10b981',
          'error': '#ef4444',
          'idle': '#6b7280',
        },
        'tool': {
          'pending': '#f59e0b',
          'success': '#10b981',
          'failed': '#ef4444',
        },
        'conn': {
          'connected': '#10b981',
          'connecting': '#f59e0b',
          'disconnected': '#ef4444',
          'reconnecting': '#3b82f6',
        },
      },
    },
  },
  plugins: [],
}
