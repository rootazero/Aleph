/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "*.html",
    "./src/**/*.rs",
  ],
  theme: {
    extend: {
      colors: {
        slate: {
          950: '#020617',
        }
      },
      fontFamily: {
        sans: ['Inter', 'system-ui', 'sans-serif'],
        mono: ['JetBrains Mono', 'monospace'],
      },
      backgroundImage: {
        'glass-gradient': 'linear-gradient(135deg, rgba(255, 255, 255, 0.05) 0%, rgba(255, 255, 255, 0) 100%)',
      },
      boxShadow: {
        'glass': '0 8px 32px 0 rgba(0, 0, 0, 0.37)',
        'neon-indigo': '0 0 20px -5px rgba(99, 102, 241, 0.5)',
        'neon-emerald': '0 0 20px -5px rgba(16, 185, 129, 0.5)',
      }
    },
  },
  plugins: [],
}
