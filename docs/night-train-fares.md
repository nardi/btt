# Night train fares in BTT

This document describes how BTT prices a night train and its accommodation. It says where the prices come from, how a ticket with a pass relates to a ticket without one, how the prices are fitted, and where the fit is weak.

## What a night train costs

A night train sells several kinds of accommodation: a seat, a berth in a couchette compartment of four or six, and a berth in a sleeper compartment of one to three. Each kind has a price, and the price depends on whether the traveller has an Interrail pass.

- **Without a pass**, one price holds the ticket and the accommodation together. The cheapest price is that of a seat, and a bed costs more by its extra over the seat.
- **With a pass**, the ticket is the pass, and the traveller pays for the reservation. A bed is a reservation, so its price holds the accommodation and the reservation. A seat reservation costs 9.90 euro on most Nightjets and 4.90 on some, and a couchette or a berth costs more.

The optimizer prices both. Single tickets for a night train get the price of the seat for the ticket, and the extra over the seat as the accommodation. A day on a pass gets the pass price of the accommodation, and no ticket.

## The ÖBB shop

The ÖBB shop sells the Nightjet and most EuroNight trains, and it lists the price of every accommodation of a train in one answer. The prices are the data of this model.

A search asks for the connections between two stations in the evening. For a connection that is a single night train, the shop answers a request for its offers with the compartments of the train, and for each compartment the price per tariff. The tariffs of a Nightjet are Sparschiene (non-flex), semi-flex and flex.

To see the price of a pass holder, the request names the Interrail card among the cards of the passenger. The shop lists it as the card `Interrail Globalpass`, with the id 2391 and no number to enter. With the card, the shop offers only the semi-flex tariff, shows the reduction `1× Interrail Globalpass`, and lists the reservation prices.

For the Nightjet 466 from Vienna to Zürich on 20 October 2026, the shop gave these prices:

| Accommodation | Sparschiene | Semi-flex | Flex | With Interrail |
|---|---|---|---|---|
| Seat | 34.90 | 69.90 | 94.90 | 9.90 |
| Couchette | 114.90 | 134.90 | 159.90 | 54.90 |
| Berth for two | 179.90 | 209.90 | 249.90 | 129.90 |

## Pass price and ticket price

A pass price is close to the semi-flex price of the bed less the semi-flex price of the seat. Over the 155 trains of the data, the pass price of a bed minus that difference has a median of minus 5 euro for both couchettes and berths. For 68 percent of the couchettes and 73 percent of the berths it lies within 10 euro, and for 98 percent or more it lies within 20 euro.

The relation is weaker for the other tariffs. Against the Sparschiene prices the difference is wider, with a median of 0 euro for couchettes and 13 euro for berths, and 42 and 44 percent within 10 euro. The Sparschiene price of a bed is not always cheaper than the semi-flex price by the same amount as the seat is. Against the flex prices the pass price is lower than the difference by a median of 10 euro for couchettes and 35 euro for berths.

So the price of a bed with a pass is about the bed ticket less the seat ticket, if both tickets are semi-flex. The relation is not exact, so the model fits the pass price by itself.

## The data

`data/fares/night_fares.jsonl` holds one line per train with its offers. The script `tools/night-train-fares.js` collects it from the shop. It searches 43 relations in both directions on three days, about 10, 24 and 45 days after the day it runs, and keeps the connections that are a single Nightjet or EuroNight with couchettes or sleepers. For each train it asks for the offers twice, without a card and with the Interrail card. Between two requests it waits two seconds. It calls `curl` for the requests, because the shop answers its TLS handshake and refuses that of Node.

A line holds the stations, the day of the collection, the departure and the train, the distance between the end stations in a straight line, and every compartment with its price per tariff and its price with the pass.

The data of 9 October 2026 holds 155 trains on 26 routes, 124 Nightjet and 31 EuroNight trains.

## The fit

The fit takes the cheapest price of each kind of accommodation from a train and fits its logarithm. A measure is a kind of accommodation with one of three bases: the price without a pass, the price with a pass, and the extra over the seat. The extra over the seat compares a bed with a seat of the same tariff and takes the lowest difference over the tariffs, because a seat and a bed of different tariffs differ by more than the bed. A seat has no extra.

Each measure has a ridge regression of the log price on two features:

$$
\log p = a + b \log\frac{d}{600\ \text{km}} + c\,[\text{EuroNight}]
$$

where $d$ is the length of the line in kilometres. The shop gives the distance in a straight line, and the planner measures rides along the line, which is about 1.3 times longer for a night train. Amsterdam to Basel is 733 km on the line against 560 km in a straight line. The data therefore use 1.3 times the straight line.

The penalty on $b$ and $c$ is 1, and the level $a$ has none. The fit needs five trains. The standard deviation of the residuals sets the range of an estimate, which holds 80 percent of the prices when they are normal, from $\exp(\pm 1.28\sigma)$ around the median.

The fits are made when the app starts, from the compiled-in data.

### Use in the optimizer

- **The ticket of a run with a night train.** `NightTrainTickets` is a price source in front of the general price model. It prices a run in which a Nightjet or EuroNight with berths is the longest ride, with the fit of the seat without a pass at the total distance of the run. Other runs go on to the general model.
- **The accommodation of a night ride.** `NightFares::accommodation_price` gives the range from the lowest couchette to the lowest berth. A ride on a pass gets the pass price. Any other ride gets the extra over the seat, because the price of its ticket holds a seat. A night train that the shop does not sell, such as the Intercités de nuit or a Trenitalia night train, has the fee of its rule in `interrail_reservations.toml` for both.
- **The worth of a pass day.** A day with a night train on the pass saves the ticket and pays the pass price of the accommodation. The optimizer counts both when it picks the days of a pass.

## Live accommodation prices

The Tickets screen asks the ÖBB shop for the accommodation of each night ride that the shop sells, as it asks for the price of a ticket. The fit is the estimate until the answer arrives, and the answer replaces it.

The optimizer attaches a request to the accommodation item of a Nightjet or EuroNight ride. The request names the train as the shop writes it, such as `NJ 466`, and whether the traveller has a pass. It carries the link to the shop search for the stations of the ride, which is also the booking link of the item. The request has no run, so the price model does not learn from an accommodation price.

`ObbService::night_offers` runs the search of the shop as it does for a ticket, picks the connection that is the single train of that name at the planned departure, and asks for its offers as an adult. For a pass holder it names the Interrail Global Pass card in the passenger, so the shop lists the pass prices. The answer is kept for half an hour.

The price shown is a range from the lowest couchette to the lowest berth, with the lowest price of each kind in its note. Without a pass, it is the extra over a seat, taken within one tariff and over the lowest tariff, as in the fit, because the ticket holds a seat. A train without seats has no such extra, and the estimate stays. With a pass, a train without beds shows the price of its seat reservation.

A lookup that cannot find the train at the planned departure leaves the estimate, and a first class traveller gets no live price, because the offers are for second class.

## Accuracy

`cargo run -p btt-fares --example night_fares_report` predicts every train from the trains of the other folds. A fold holds all days and both directions of a route, so a route is never in the fitting and the test parts at once. The error is the factor between the estimate and the price, and half the prices stay within the median error.

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

The constant price is the median of the other folds, with no feature. The report also shows the general price model on the cheapest seat of the same trains, which is off by a median factor of 3.69. It prices a night train by its distance, as it prices a day train, and the price of a night train hardly depends on the distance. A seat costs 25 to 50 euro for the Nightjets in the data, whether the route is 500 or 1200 kilometres long.

For the Nightjet 403 from Amsterdam to Basel, the general model gave a range of 107 to 422 euro and DB showed 52.99 euro. The fit gives 26 to 45 euro.

## Limitations

- **Demand.** A night train sells its cheapest seats first, and the prices of one route differ from day to day. The data hold the prices of one day of collection for three days of departure. Seats cost the same median 34.90 euro at every horizon, and couchettes cost 94.90 euro for a departure 10 days ahead and 78 euro for one 24 or 45 days ahead, from 46 to 55 trains per horizon. The fit uses no feature of the date, because the days ahead and the weekday added little in a test by cross-validation.
- **Trains without seats.** Some night trains do not sell seats, such as the Nightjet from Amsterdam to Zürich. The estimate of the ticket then still follows the seats of the other trains.
- **One shop.** The model covers the trains that the ÖBB shop sells. The distance from the straight line is an approximation that is off by a tenth for routes with a long detour.
- **One traveller.** The prices are for one adult. A compartment shared by several travellers is priced per person, and the model takes the cheapest.
- **Second class.** A first class ticket gets the same estimate.
