import React from 'react';
import ReactDOM from 'react-dom/client';
import '@/styles/globals.css';
import '@/lib/i18n';
import { HaloWindow } from './HaloWindow';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <HaloWindow />
  </React.StrictMode>
);
