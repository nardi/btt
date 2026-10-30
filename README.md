<div align="center">

  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/logo-dark.svg">
    <img src="docs/logo-light.svg" alt="BigTrainTrip" width="180">
  </picture>

  <h2 align="center">
  BigTrainTrip - Plan long train trips across Europe and see what they cost
  </h2>

</div>

## Introduction

BigTrainTrip (BTT) is a desktop app for planning a train trip that visits several places. You list the places and how long you want to stay in each. The app picks a journey for every leg, prefers night trains where they save you a hotel, and estimates what the tickets will cost.

1. It plans the whole trip at once. Each leg gets the journey that fits your time windows, your tolerance for changing trains and, if you set one, your bedtime and wake-up time.
2. It shows the alternatives for every leg, so you can swap a journey and see the effect on the rest of the trip.
3. It prices the trip. Prices come from published tariffs and from the live prices of booking sites, with a statistical model for everything else. The app can also work out whether an Interrail pass is cheaper than single tickets.
4. It keeps everything on your computer. There is no account, and your trips never leave the app unless you share them.

Download an installer from the [releases page](https://github.com/nardi/btt/releases). There are installers for Windows, macOS and Linux. They are not signed, so Windows and macOS will ask you to confirm that you want to open them.

### Portable version

The releases page also has a portable archive for each system. It needs no installation and no admin rights, so it runs from a USB stick or a folder you cannot install into. Unpack the archive and start `BigTrainTrip` (`BigTrainTrip.exe` on Windows).

The archive holds a file called `portable.txt` next to the app. While that file is there, the app keeps your trips, travellers, timetable and learned prices in a `data` folder beside it, and it leaves your user profile alone. To move to another computer, copy the whole folder. If you delete `portable.txt`, the app goes back to your user profile and ignores the `data` folder.

The installed app can become portable too. Put an empty `portable.txt` next to the executable, next to the `.app` bundle on macOS, or next to the `.AppImage` file on Linux.

The web view of the operating system still keeps its own cache and cookies in your user profile, for example the cookies of a booking site you opened from the app. Windows needs the WebView2 runtime, which Windows 11 and recent Windows 10 builds include. The Linux archive needs WebKitGTK 4.1 to be installed, for example the `libwebkit2gtk-4.1-0` package on Debian and Ubuntu.

### Why?

Booking sites answer one question at a time, such as "when is the next train from Amsterdam to Vienna?". A trip across Europe is a chain of those questions, and the answers depend on each other. A late arrival in one city can make a night train in the next one impossible, and a cheap ticket on the wrong day can cost you a night in a hotel. BTT looks at the chain as a whole, so that you compare complete trips instead of single connections.

## Using the app

A new trip starts with a list of places. Type a city or a station and the app suggests matches, then set the number of nights for each stay. Add travellers if you want prices for more than one adult in second class, and say which discount cards or Interrail passes they own.

The planner then fills in a journey for each leg. The map shows the route, and the itinerary lists the trains, the changes and the time you spend in each place. Open the alternatives of a leg to pick another journey, or change the preferences of the trip to plan again.

The tickets screen shows what you need to buy. It groups the rides that one ticket can cover, links to the booking site that sells each ticket, and shows the live price when the site has one. Prices without a live source are estimates, and the app shows them as a range.

### Sharing a trip

The share dialog turns a trip into a link, a QR code or a file. Anyone who opens a link or scans the code sees the trip in the [web viewer](https://nardi.github.io/btt/), without installing anything. The link holds the whole trip after the `#` sign, so no server ever receives it. Travellers' names and ages are never included.

Long trips make long links. The dialog lets you leave out the alternatives, the price estimates and the names of places to keep the link and the QR code short, and it can write the link in a compact alphabet that halves its length. A file always holds the complete trip.

## Where the data comes from

The trains and times come from public timetable feeds of railway operators and transport authorities, mostly collected by the [Transitous](https://transitous.org) project. The app downloads a timetable file from the releases of this repository. A GitHub workflow rebuilds that file every Monday and a build is valid for 90 days. The About page of the app lists every feed with its license.

Besides the timetable, the app makes two kinds of requests. It loads map tiles from [OpenFreeMap](https://openfreemap.org), and it asks booking sites for the live price of a ticket when you price a trip.

## Status

BTT is in early development. The first release is `v0.1.0`, so expect rough edges and changes to the stored format of trips. Links written by older versions keep opening in the viewer.

Timetable coverage is wide but uneven. The data covers most of Europe, and the quality of prices differs per operator. Night trains have their own price model because they cost differently from day trains.

## Development

The app is a [Tauri](https://tauri.app) program with a Rust backend and a Svelte frontend. You need Rust 1.95 or newer and Node 24. On Linux you also need the [system libraries Tauri depends on](https://tauri.app/start/prerequisites/#linux).

Install the dependencies and start the app:

```bash
npm ci
npm run dev
```

Run the checks that CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
npm test
```

### Code structure

The Rust code is a workspace of small crates, and the frontend is an npm workspace of three packages.

* `btt-model` holds the types of trips, journeys and tickets that the other crates share.
* `btt-builder` turns timetable feeds into the timetable file, and `btt-timetable` reads that file.
* `btt-router` and `btt-planner` search the trains and choose a journey for every leg.
* `btt-fares`, `btt-quotes` and `btt-ticketing` estimate prices, read live prices and find the cheapest tickets.
* `btt-geo` and `btt-storage` handle places and the local database.
* `src-tauri` is the app itself, which exposes the crates to the frontend. The TypeScript types in `ui/shared/src/bindings.ts` are generated from the Rust types.
* `ui/app` is the desktop interface, `ui/viewer` is the web viewer and `ui/shared` holds what they have in common.

### Documentation

* [Journey planning](docs/journey-planning.md) explains how the planner chooses a journey.
* [Price model](docs/price-model.md) explains how the app estimates ticket prices.
* [Night train fares](docs/night-train-fares.md) explains how the app prices night trains.

## License

BTT is released under the [Hippocratic License 3.0](LICENSE.md).
