import React from 'react';
import ReactDOM from 'react-dom/client';
import '@/styles/globals.css';

// Conversation window - placeholder for Phase 2+
function ConversationWindow() {
  return (
    <div className="flex h-screen items-center justify-center bg-background text-foreground">
      <div className="text-center">
        <h1 className="text-heading mb-2">Conversation</h1>
        <p className="text-muted-foreground">Coming in Phase 2...</p>
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ConversationWindow />
  </React.StrictMode>
);
