import {
  ChevronLeft,
  ChevronRight,
  Crosshair,
  FolderOpen,
  History,
  Info,
  Check,
  Edit3,
  Layers3,
  LogIn,
  LogOut,
  ListChecks,
  Move,
  PanelRightClose,
  RotateCcw,
  RefreshCw,
  Ruler,
  Search,
  ScanSearch,
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
      History,
      Info,
      Check,
      Edit3,
      Layers3,
      LogIn,
      LogOut,
      ListChecks,
      Move,
      PanelRightClose,
      RotateCcw,
      RefreshCw,
      Ruler,
      Search,
      ScanSearch,
      SlidersHorizontal,
      Users,
      X,
    },
  });
  new App();
});
