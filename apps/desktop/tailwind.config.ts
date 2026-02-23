import type { Config } from 'tailwindcss';

const config: Config = {
  darkMode: 'class',
  content: ['./index.html', './halo.html', './settings.html', './conversation.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      // Spacing (aligned with macOS DesignTokens.Spacing)
      spacing: {
        xs: '4px',   // tight spacing between related elements
        sm: '8px',   // compact spacing for dense UIs
        md: '16px',  // standard spacing between elements
        lg: '24px',  // comfortable spacing for sections
        xl: '32px',  // loose spacing for major sections
      },

      // Border Radius (aligned with macOS DesignTokens.CornerRadius - concentric system)
      borderRadius: {
        // Concentric radius system
        xs: '4px',   // minimum radius
        sm: '6px',   // small controls, buttons
        md: '10px',  // cards, inputs
        lg: '12px',  // content areas, windows
        xl: '16px',  // large containers, modals
        // Legacy aliases (for backward compatibility)
        small: '6px',
        medium: '10px',
        large: '12px',
        card: '10px',
      },

      // Font Size (aligned with macOS DesignTokens.Typography)
      fontSize: {
        title: ['22px', { lineHeight: '1.3', fontWeight: '600' }],    // page titles
        heading: ['17px', { lineHeight: '1.4', fontWeight: '500' }],  // section headers
        body: ['14px', { lineHeight: '1.5', fontWeight: '400' }],     // body text
        caption: ['12px', { lineHeight: '1.4', fontWeight: '400' }],  // supporting text
        code: ['13px', { lineHeight: '1.4' }],                        // monospace code
      },

      fontFamily: {
        sans: [
          '-apple-system',
          'BlinkMacSystemFont',
          'Segoe UI',
          'Roboto',
          'Oxygen',
          'Ubuntu',
          'sans-serif',
        ],
        mono: ['SF Mono', 'Consolas', 'Liberation Mono', 'monospace'],
      },

      // Colors (semantic, using CSS variables)
      colors: {
        border: 'hsl(var(--border))',
        input: 'hsl(var(--input))',
        ring: 'hsl(var(--ring))',
        background: 'hsl(var(--background))',
        foreground: 'hsl(var(--foreground))',
        primary: {
          DEFAULT: 'hsl(var(--primary))',
          foreground: 'hsl(var(--primary-foreground))',
        },
        secondary: {
          DEFAULT: 'hsl(var(--secondary))',
          foreground: 'hsl(var(--secondary-foreground))',
        },
        destructive: {
          DEFAULT: 'hsl(var(--destructive))',
          foreground: 'hsl(var(--destructive-foreground))',
        },
        muted: {
          DEFAULT: 'hsl(var(--muted))',
          foreground: 'hsl(var(--muted-foreground))',
        },
        accent: {
          DEFAULT: 'hsl(var(--accent))',
          foreground: 'hsl(var(--accent-foreground))',
        },
        card: {
          DEFAULT: 'hsl(var(--card))',
          foreground: 'hsl(var(--card-foreground))',
        },
        // Custom semantic colors
        success: 'hsl(var(--success))',
        warning: 'hsl(var(--warning))',
        error: 'hsl(var(--error))',
        info: 'hsl(var(--info))',
      },

      // Animations
      animation: {
        'pulse-slow': 'pulse 0.8s ease-in-out infinite',
        'spin-slow': 'spin 1s linear infinite',
        'fade-in': 'fadeIn 0.2s ease-out',
        'scale-in': 'scaleIn 0.2s ease-out',
        'slide-in-bottom': 'slideInBottom 0.2s ease-out',
      },

      keyframes: {
        fadeIn: {
          '0%': { opacity: '0' },
          '100%': { opacity: '1' },
        },
        scaleIn: {
          '0%': { transform: 'scale(0.95)', opacity: '0' },
          '100%': { transform: 'scale(1)', opacity: '1' },
        },
        slideInBottom: {
          '0%': { transform: 'translateY(10px)', opacity: '0' },
          '100%': { transform: 'translateY(0)', opacity: '1' },
        },
      },
    },
  },
  plugins: [require('tailwindcss-animate')],
};

export default config;
