# Night train fares in BTT

This document explains how BTT prices a night train and the seat or bed the traveller spends the night in. A night train does not price like a day train, so the general price model of `docs/price-model.md` gets it badly wrong, and we fit night trains separately. We first look at what a night train costs and how a ticket with a pass relates to one without. Then we describe where the prices come from, how we fit them, how the app uses the fit and how well it does. The ÖBB shop sells the Nightjet and the EuroNight. European Sleeper and Snälltåget have booking sites of their own, so we fit each operator to its own offers.

## What a night train costs

A night train sells several kinds of accommodation. The traveller can take a seat, a berth in a couchette compartment of four or six, or a berth in a sleeper compartment of one to three. Each kind has its own price, and that price depends on whether the traveller has an Interrail pass.

- **Without a pass**, one price covers the ticket and the accommodation together. The cheapest price is that of a seat, and a bed costs more by its extra over the seat.
- **With a pass**, the pass is the ticket, and the traveller only pays for the reservation. A bed counts as a reservation, so its price covers both the accommodation and the reservation. A seat reservation costs 9.90 euro on most Nightjets and 4.90 on some, and a couchette or a berth costs more.

The optimizer has to price both cases, and it splits the price the same way in each. A single ticket for a night train gets the price of the seat, and the extra over the seat becomes the accommodation. A day on a pass gets the pass price of the accommodation and no ticket. European Sleeper is the exception, because it sells a ticket together with its place. Its ticket therefore holds the bed, and its accommodation without a pass costs 0.

## The ÖBB shop

The ÖBB shop sells the Nightjet and most EuroNight trains. It lists the price of every accommodation of a train in one answer, which makes it a good source of data for the fit.

A search asks for the connections between two stations in the evening. For a connection that is a single night train, we then ask the shop for its offers. The shop answers with the compartments of the train and the price of each compartment per tariff. A Nightjet has three tariffs, which are Sparschiene (non-flex), semi-flex and flex.

How do we see what a pass holder pays? The request names the Interrail card among the cards of the passenger. The shop lists it as the card `Interrail Globalpass`, with the id 2391 and no number to enter. With the card, the shop offers only the semi-flex tariff, shows the reduction `1× Interrail Globalpass` and lists the reservation prices.

As an example, the shop gave these prices for the Nightjet 466 from Vienna to Zürich on 20 October 2026:

| Accommodation | Sparschiene | Semi-flex | Flex | With Interrail |
|---|---|---|---|---|
| Seat | 34.90 | 69.90 | 94.90 | 9.90 |
| Couchette | 114.90 | 134.90 | 159.90 | 54.90 |
| Berth for two | 179.90 | 209.90 | 249.90 | 129.90 |

## Pass price and ticket price

The table suggests a simple rule. The pass replaces the seat ticket, so the pass price of a bed should be about the bed ticket minus the seat ticket. How well does that hold? For the semi-flex tariff it holds fairly well. Over the 155 trains of the data, the pass price of a bed minus the difference of the semi-flex prices has a median of minus 5 euro, for couchettes and for berths alike. It lies within 10 euro for 68 percent of the couchettes and 73 percent of the berths, and within 20 euro for 98 percent or more.

For the other tariffs the rule holds less well. Against the Sparschiene prices the difference has a median of 0 euro for couchettes and 13 euro for berths, and only 42 and 44 percent lie within 10 euro. The reason is that a Sparschiene bed is not always cheaper than a semi-flex bed by the same amount as a Sparschiene seat is. Against the flex prices, the pass price lies below the difference by a median of 10 euro for couchettes and 35 euro for berths.

So the price of a bed with a pass is about the bed ticket less the seat ticket, as long as both tickets are semi-flex. The rule is not exact, though, so the model fits the pass price on its own instead of deriving it.

## The data

`data/fares/night_fares.jsonl` holds one line per train with its offers, and the script `tools/night-train-fares.js` collects it from the shop. The script searches 43 relations in both directions on three days, about 10, 24 and 45 days after the day it runs. It keeps the connections that are a single Nightjet or EuroNight with couchettes or sleepers. For each train it asks for the offers twice, once without a card and once with the Interrail card, and it waits two seconds between two requests. It calls `curl` for the requests, because the shop accepts the TLS handshake of `curl` and refuses that of Node.

A line holds the stations, the day of the collection, the departure and the train, and the distance between the end stations in a straight line. It also holds every compartment with its price per tariff and its price with the pass.

The data of 9 October 2026 hold 155 trains on 26 routes. Of these, 124 are Nightjets and 31 are EuroNight trains.

## The fit

From each train, the fit takes the cheapest price of each kind of accommodation and fits its logarithm. We call each quantity the fit predicts a measure. A measure is a kind of accommodation together with one of three bases, which are the price without a pass, the price with a pass and the extra over the seat.

The extra over the seat compares a bed with a seat of the same tariff and takes the lowest difference over the tariffs. Why not simply subtract the cheapest seat from the cheapest bed? Because a seat and a bed of different tariffs differ by more than the bed is worth. A seat has no extra.

Each measure gets a ridge regression of the log price on two features:

$$
\log p = a + b \log\frac{d}{600\ \text{km}} + c\,[\text{EuroNight}]
$$

where $d$ is the length of the line in kilometres. The distance needs one correction. The shop gives the distance in a straight line, but the planner measures rides along the line, which for a night train is about 1.3 times longer. Amsterdam to Basel, for example, is 733 km on the line against 560 km in a straight line. The data therefore use 1.3 times the straight line, so the fit and the planner measure the same thing.

The penalty on $b$ and $c$ is 1, and the level $a$ has no penalty. With few trains, the penalty keeps the slope and the EuroNight offset from chasing noise. The fit needs at least five trains. The standard deviation $\sigma$ of the residuals sets the range of an estimate, from $\exp(\pm 1.28\sigma)$ around the median. If the residuals are normal, that range holds 80 percent of the prices.

The app makes the fits when it starts, from the compiled-in data.

### Use in the optimizer

The fit enters the optimizer in three places.

- **The ticket of a run with a night train.** `NightTrainTickets` is a price source in front of the general price model. It prices a run in which a night train with berths is the longest ride, with the fit of the seat without a pass at the total distance of the run. `NightOperator` says whose offers the fit comes from. Other runs go on to the general model.
- **The accommodation of a night ride.** `NightFares::accommodation_price` gives the range from the lowest couchette to the lowest berth. A ride on a pass gets the pass price. Any other ride gets the extra over the seat, because the price of its ticket already holds a seat. A European Sleeper ride without a pass gets 0, and `NightTrainTickets` adds the range of the bed to the seat price of its ticket instead. A night train that the shop does not sell, such as the Intercités de nuit or a Trenitalia night train, gets the fee of its rule in `interrail_reservations.toml` for both.
- **The worth of a pass day.** A day with a night train on the pass saves the ticket but pays the pass price of the accommodation. The optimizer counts both when it picks the days of a pass.

## Other operators

`NightOperator` names the operators that have fits of their own. A train belongs to the ÖBB shop when its category is `NJ`, `Nightjet`, `EN` or `EuroNight`, whoever runs it. The trains of European Sleeper and Snälltåget belong to those operators by their id in `operators.toml`. Every operator has the same nine measures and the same features as above, fitted to its own file of offers. Only the ÖBB shop uses the EuroNight offset.

### European Sleeper

The booking service of European Sleeper answers plain requests without a login. A search for a day lists the trains between two stations, and the availability of a train lists the lowest price of every kind of place. A place is a seat, a bed in a shared compartment of five, or a bed in a compartment of two or three. The script leaves out a private compartment, because the service prices it for the whole compartment and not per traveller. The service also has the Interrail reservation as a traveller type of its own, which gives us the price of a pass holder.

`tools/european-sleeper-fares.js` collects the offers for pairs of stations in both directions, on days spread over the next months. For each train it asks for the availability as an adult and as a pass holder. `data/fares/european_sleeper_fares.jsonl` holds the answers in the shape of `night_fares.jsonl`, with the three fare types of the service as tariffs. `data/fares/european_sleeper_stations.csv` holds the stations with the codes of the service and their positions.

The prices of European Sleeper differ from those of the ÖBB shop in two ways. First, they follow the day more than the distance. They are lower far ahead and higher near the departure or in the holidays, and a seat between Amsterdam and Berlin costs anything from 19.99 to 169.99 euro in the offers. Second, the pass price of a place is the same on every train of a route. The fit has no feature of the day, so its range is wide.

### Snälltåget

The site of Snälltåget hands anyone a token, and its journey search lists the bundles of a route with their prices in krona. `tools/snalltaget-fares.js` keeps the routes that sell a berth, converts the prices at the rate of the ECB on the day and writes `data/fares/snalltaget_fares.jsonl`. A seat is the `SP` family, a berth in a shared compartment the `NTB` family, and a private compartment the `NTPC` family. The search holds no price for a pass holder. The pass measures therefore have no fit, and a pass holder gets the fee of the rule.

## Live accommodation prices

The fit is only an estimate, and the shop knows the real price. The Tickets screen therefore asks the ÖBB shop for the accommodation of each night ride that the shop sells, in the same way as it asks for the price of a ticket. The fit stands in until the answer arrives, and the answer then replaces it.

The optimizer attaches a request to the accommodation item of a Nightjet or EuroNight ride. The request names the train as the shop writes it, such as `NJ 466`, and says whether the traveller has a pass. It carries the link to the shop search for the stations of the ride, which is also the booking link of the item. The request has no run, so the price model does not learn from an accommodation price.

`ObbService::night_offers` runs the search of the shop as it does for a ticket. It picks the connection that is the single train of that name at the planned departure, and it asks for the offers of that connection as an adult. For a pass holder it names the Interrail Global Pass card in the passenger, so that the shop lists the pass prices. The app keeps the answer for half an hour.

The price shown is a range from the lowest couchette to the lowest berth, with the lowest price of each kind in its note. Without a pass, the price is the extra over a seat, because the ticket already holds a seat. As in the fit, the extra is taken within one tariff and then over the lowest tariff. A train without seats has no such extra, and the estimate stays. With a pass, a train without beds shows the price of its seat reservation.

When the lookup cannot find the train at the planned departure, the estimate stays. A first class traveller gets no live price, because the offers are for second class. The shop has listed a train in one search and left it out of the next, so when a search finds no such train, the lookup tries once more after two seconds.

### Trains without seats

Some Nightjets sell beds only, such as the one from Amsterdam to Zürich. Without a pass, the extra over a seat then has nothing to start from. The shop prices a bed at the whole ticket, while the plan holds a ticket item priced as a seat. Subtracting a seat is still right, but the price of that seat has to come from somewhere else. The request therefore carries the estimated price of a seat, and the lookup subtracts it from the lowest bed price.

## European Sleeper live prices

For the ticket and the accommodation of a European Sleeper ride, the app asks the booking service for the train that leaves within five minutes of the planned time. The answer holds the prices of an adult and of a pass holder for every kind of place, and the app keeps it for half an hour.

- **The ticket.** The price is a range from the lowest bed in a shared compartment to the lowest bed in a small compartment, each with its ticket, because the ticket of a night ride holds the bed. A run that is known to have no berths gets the cheapest place the train sells, which is usually a seat.
- **The accommodation without a pass.** The price is 0 and the item has no lookup, because the ticket already holds the bed.
- **The accommodation with a pass.** The price is the same range, for the reservation of the place. When the place with a regular ticket costs less than its reservation, the lower price counts and the note says so.
- **Last minute fares.** The main site lists a fare for a bed in a shared compartment, with the ticket, for trains one to three weeks away. On 10 October 2026 its pages listed 40 euro per traveller. For a train within five weeks of today, the lookup reads the four pages of the site and looks for a fare between its stations on its day. The fare counts as a place with its ticket. It can therefore be the cheapest ticket, and it can be lower than the reservation of a pass holder. Child discounts and the Interrail reservation do not apply to it. The pages also sell a private compartment for up to five travellers, which the lookup leaves out.

## Accuracy

`cargo run -p btt-fares --example night_fares_report` predicts every train from the trains of the other folds. A fold holds all days and both directions of a route, so a route is never in the fitting part and the test part at once. The error is the factor between the estimate and the price, and half the prices stay within the median error. Do the distance and the EuroNight offset help at all? To find out, the report also shows a constant price, which is the median of the other folds with no feature.

| Price | Trains | Median error | Constant price | Inside the range |
|---|---|---|---|---|
| Seat, without a pass | 115 | 1.07 | 1.29 | 69 percent |
| Seat, with a pass | 115 | 1.12 | 1.00 | 82 percent |
| Couchette, without a pass | 148 | 1.23 | 1.20 | 72 percent |
| Couchette, with a pass | 147 | 1.20 | 1.25 | 73 percent |
| Couchette, extra over the seat | 115 | 1.30 | 1.43 | 77 percent |
| Berth, without a pass | 151 | 1.18 | 1.17 | 77 percent |
| Berth, with a pass | 150 | 1.13 | 1.18 | 78 percent |
| Berth, extra over the seat | 111 | 1.18 | 1.30 | 83 percent |

The fit beats the constant price clearly for the seat without a pass and for the two extras over the seat. For the beds without a pass the two are about even. For the seat with a pass the constant wins, because the seat reservation costs 9.90 euro on most trains whatever the route.

The report also shows the general price model on the cheapest seat of the same trains, and it is off by a median factor of 3.69. The general model prices a night train by its distance, as it prices a day train, but the price of a night train hardly depends on the distance. A seat costs 25 to 50 euro for the Nightjets in the data, whether the route is 500 or 1200 kilometres long. For the Nightjet 403 from Amsterdam to Basel, the general model gave a range of 107 to 422 euro, and DB showed 52.99 euro. The fit gives 26 to 45 euro.

### Accuracy for the other operators

The same report runs on the files of the other operators, for example with `cargo run -p btt-fares --example night_fares_report -- data/fares/european_sleeper_fares.jsonl`. The offers of European Sleeper hold 397 trains on 23 routes, collected on 10 October 2026 for departures 7 to 170 days ahead. The offers of Snälltåget hold 113 trains on 11 routes, without pass prices.

| Price | European Sleeper trains | Median error | Inside the range | Snälltåget trains | Median error | Inside the range |
|---|---|---|---|---|---|---|
| Seat, without a pass | 388 | 1.22 | 87 percent | 107 | 1.27 | 77 percent |
| Seat, with a pass | 385 | 1.13 | 59 percent | none | | |
| Couchette, without a pass | 339 | 1.28 | 80 percent | 47 | 1.10 | 83 percent |
| Couchette, with a pass | 336 | 1.09 | 82 percent | none | | |
| Couchette, extra over the seat | 317 | 1.47 | 81 percent | 43 | 1.22 | 77 percent |
| Berth, without a pass | 331 | 1.12 | 81 percent | 101 | 1.27 | 72 percent |
| Berth, with a pass | 328 | 1.07 | 95 percent | none | | |
| Berth, extra over the seat | 327 | 1.16 | 80 percent | 98 | 1.32 | 69 percent |

On the seats of European Sleeper the general price model is off by a median factor of 2.17, against 1.22 for the fit. A constant price comes close to the fit, at 1.33 for the seat, because the day moves the price more than the distance does. The pass prices of European Sleeper vary little on a route. The range of the fit is therefore narrow, and it misses 41 percent of the pass seat prices.

## Limitations

- **Demand.** A night train sells its cheapest seats first, so the prices of one route differ from day to day. The data hold the prices of one day of collection for three days of departure. Seats cost the same median 34.90 euro at every horizon. Couchettes cost 94.90 euro for a departure 10 days ahead and 78 euro for one 24 or 45 days ahead, with 46 to 55 trains per horizon. The fit uses no feature of the date, because the days ahead and the weekday added little in a test by cross-validation.
- **Trains without seats.** Some night trains do not sell seats, such as the Nightjet from Amsterdam to Zürich. The estimate of their ticket still follows the seats of the other trains.
- **Three sources.** The model covers the trains of the ÖBB shop, of European Sleeper and of Snälltåget. Other night trains, such as the Intercités de nuit, the Trenitalia night trains and the Caledonian Sleeper, use the general model and the fee of their rule. The distance from the straight line is an approximation, and it is off by a tenth for routes with a long detour.
- **The day.** The prices of European Sleeper and Snälltåget follow the days ahead and the season, and the fits ignore both. `tools/night-fares-day-features.js` tests what the days ahead, the weekday and the weeks around Christmas would add, by cross-validation. For European Sleeper the mean error of the log price falls from 0.272 to 0.197 for the couchette and from 0.288 to 0.251 for the seat. For Snälltåget it does not fall. All offers come from one day of collection, so the test cannot tell the days ahead apart from the date of the departure. The live price replaces the estimate when it arrives.
- **One traveller.** The prices are for one adult. A compartment shared by several travellers is priced per person, and the model takes the cheapest.
- **Second class.** A first class ticket gets the same estimate.
