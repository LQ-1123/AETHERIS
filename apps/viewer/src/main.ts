import {
  ChevronLeft,
  ChevronRight,
  FolderOpen,
  Info,
  Move,
  PanelRightClose,
  RotateCcw,
  Ruler,
  SlidersHorizontal,
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
      FolderOpen,
      Info,
      Move,
      PanelRightClose,
      RotateCcw,
      Ruler,
      SlidersHorizontal,
      X,
    },
  });
  new App();
});
