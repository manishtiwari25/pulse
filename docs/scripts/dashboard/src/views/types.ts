export interface ChartPrefs {
  range: string;
  by: string;
}

// Every view gets the poller's revision (a bump means the snapshot changed),
// whether this draw is a navigation (animate) or a quiet redraw, the shared
// chart preferences, and a way to report the stamp it saw.
export interface ViewProps {
  revision: number;
  animate: boolean;
  chart: ChartPrefs;
  onChartPrefs: (p: Partial<ChartPrefs>) => void;
  onStamp: (stamp: string) => void;
  arg?: string;
}
