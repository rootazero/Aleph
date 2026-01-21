import React from 'react';
import ReactDOM from 'react-dom/client';
import '@/styles/globals.css';
import { SettingsWindow } from './SettingsWindow';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <SettingsWindow />
  </React.StrictMode>
);
