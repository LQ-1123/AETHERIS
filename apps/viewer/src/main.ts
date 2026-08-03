import {
  ChevronLeft,
  ChevronRight,
  Crosshair,
  FolderOpen,
  Info,
  Layers3,
  LogIn,
  LogOut,
  Move,
  PanelRightClose,
  RotateCcw,
  RefreshCw,
  Ruler,
  Search,
  SlidersHorizontal,
  Users,
  X,
  createIcons,
} from 'lucide';
import { App } from './app';
import './styles.css';

window.addEventListener('DOMContentLoaded', () => {
  createIcons({
    icons: {
      ChevronLeft,
      ChevronRight,
      Crosshair,
      FolderOpen,
      Info,
      Layers3,
      LogIn,
      LogOut,
      Move,
      PanelRightClose,
      RotateCcw,
      RefreshCw,
      Ruler,
      Search,
      SlidersHorizontal,
      Users,
      X,
    },
  });
  new App();
});
