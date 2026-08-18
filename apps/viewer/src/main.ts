/// <reference types="vite/client" />

import {
  ArchiveRestore,
  ArrowLeft,
  Blend,
  Bold,
  BookmarkPlus,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ClipboardList,
  Crosshair,
  Circle,
  Contrast,
  DraftingCompass,
  Ellipsis,
  Eye,
  Eraser,
  FlipHorizontal2,
  FlipVertical2,
  FileText,
  FolderOpen,
  History,
  Image,
  Info,
  Italic,
  Check,
  Edit3,
  Layers3,
  LayoutGrid,
  Link,
  List,
  ListOrdered,
  LogIn,
  LogOut,
  ListChecks,
  Move,
  MousePointer2,
  Network,
  PanelRightClose,
  Paintbrush,
  Pencil,
  Pause,
  Play,
  RotateCcw,
  RotateCw,
  Redo2,
  RefreshCw,
  Ruler,
  Search,
  ScanSearch,
  ScanLine,
  Save,
  Share2,
  ShieldCheck,
  SlidersHorizontal,
  Square,
  Sparkles,
  Sun,
  Trash2,
  Underline,
  Undo2,
  Users,
  X,
  createIcons,
} from 'lucide';
import { App } from './app';
import './styles.css';

window.addEventListener('DOMContentLoaded', () => {
  createIcons({
    icons: {
      ArchiveRestore,
      ArrowLeft,
      Blend,
      Bold,
      BookmarkPlus,
      ChevronDown,
      ChevronLeft,
      ChevronRight,
      ClipboardList,
      Crosshair,
      Circle,
      Contrast,
      DraftingCompass,
      Ellipsis,
      Eye,
      Eraser,
      FlipHorizontal2,
      FlipVertical2,
      FileText,
      FolderOpen,
      History,
      Image,
      Info,
      Italic,
      Check,
      Edit3,
      Layers3,
      LayoutGrid,
      Link,
      List,
      ListOrdered,
      LogIn,
      LogOut,
      ListChecks,
      Move,
      MousePointer2,
      Network,
      PanelRightClose,
      Paintbrush,
      Pencil,
      Pause,
      Play,
      RotateCcw,
      RotateCw,
      Redo2,
      RefreshCw,
      Ruler,
      Search,
      ScanSearch,
      ScanLine,
      Save,
      Share2,
      ShieldCheck,
      SlidersHorizontal,
      Square,
      Sparkles,
      Sun,
      Trash2,
      Underline,
      Undo2,
      Users,
      X,
    },
  });

  if (new URLSearchParams(window.location.search).get('mode') === 'report') {
    // 报告独立小窗：隐藏登录屏与主阅片器，只初始化报告 UI
    const login = document.getElementById('login-screen');
    const shell = document.getElementById('app-shell');
    if (login) login.hidden = true;
    if (shell) shell.style.display = 'none';
    const root = document.getElementById('report-window-root');
    if (root) root.hidden = false;
    void import('./report-window').then(({ ReportWindow }) => new ReportWindow().init());
    return;
  }
  if (import.meta.env.DEV && import.meta.env.VITE_QUEUE_ACCEPTANCE === '1') {
    void import('./queue-acceptance').then(({ installQueueAcceptanceMock }) => {
      installQueueAcceptanceMock();
      new App();
    });
    return;
  }
  if (import.meta.env.DEV && import.meta.env.VITE_PASSWORD_RESET_ACCEPTANCE === '1') {
    void import('./password-reset-acceptance').then(({ installPasswordResetAcceptanceMock }) => {
      installPasswordResetAcceptanceMock();
      new App();
    });
    return;
  }
  new App();
});
