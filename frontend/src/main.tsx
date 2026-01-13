// 应用入口
// Input: React, App
// Output: DOM 渲染
// Pos: 应用启动点

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { ToastContainer, Flip } from 'react-toastify';

import './assets/style/index.css';
import 'react-toastify/dist/ReactToastify.css';
import App from './App.tsx';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
    <ToastContainer
      position="top-right"
      autoClose={3000}
      hideProgressBar={false}
      newestOnTop
      closeOnClick
      rtl={false}
      pauseOnFocusLoss
      draggable
      pauseOnHover
      theme="colored"
      transition={Flip}
    />
  </StrictMode>
);
