import { render } from 'preact';
import { App } from './app';
import { TooltipProvider } from './components/ui/tooltip';
import { initTheme } from './lib/theme';
import './styles/tokens.css';

initTheme();

const root = document.getElementById('app');
if (root) {
  render(
    <TooltipProvider delayDuration={250} skipDelayDuration={400}>
      <App />
    </TooltipProvider>,
    root,
  );
}
