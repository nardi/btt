// The features that differ between the desktop app and the web app.

/** The features that only some builds of the app have. */
export interface AppFeatures {
  /**
   * Whether the app looks up live prices on booking sites. Booking sites do not accept requests
   * from a web page, so the web app shows estimates only.
   */
  livePrices: boolean;
  /** Whether the app can write a problem report. */
  problemReports: boolean;
  /** Whether the traveller can choose where the app keeps its data, which is portable mode. */
  dataLocation: boolean;
  /**
   * Whether the app runs in a browser. The browser stores the data, and a saved file goes to the
   * downloads of the browser.
   */
  runsInBrowser: boolean;
}

/** The features of the desktop app, which has all of them. */
const desktopFeatures: AppFeatures = {
  livePrices: true,
  problemReports: true,
  dataLocation: true,
  runsInBrowser: false,
};

let currentFeatures: AppFeatures = desktopFeatures;

/** Returns the features of this build. */
export function appFeatures(): AppFeatures {
  return currentFeatures;
}

/** Sets the features of this build. Call it once, before the app mounts. */
export function setAppFeatures(features: Partial<AppFeatures>): void {
  currentFeatures = { ...desktopFeatures, ...features };
}
