import React from 'react';
import ReactDOM from 'react-dom/client';
import '@/styles/globals.css';

// Main entry point - redirects based on context
// In Tauri, each window has its own HTML entry point
function App() {
  return (
    <div className="flex h-screen items-center justify-center bg-background text-foreground">
      <div className="text-center">
        <h1 className="text-title mb-4">Aether</h1>
        <p className="text-muted-foreground">
          This is the main entry point. In production, use the system tray to access Aether.
        </p>
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
