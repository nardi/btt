# Journey planning in BTT

This document explains how BTT picks the trains for a trip. We first state the task as an optimization problem, because that makes clear what a good plan is. Then we describe how the planner solves the problem and how its method relates to existing work on public transit routing. The last part lists the places where the method departs from the stated problem.

## The problem

A trip is a list of places. Between two consecutive places lies a leg, and each place in the middle has a stay. A stay is either a number of nights or a same-day stop of a few hours. The traveller sets time windows for leaving and arriving, a transfer aversion and a minimum time for changing trains. For a leg with a night train, the traveller also sets a bedtime and a wake-up time. The planner has to pick one journey per leg, where a journey is a sequence of train rides with the changes and walks between them.

### Time-expanded graph

What does it mean to pick the best journey? We can make that precise by writing a leg as a shortest path problem on a time-expanded graph. The nodes of the graph are pairs of a station and a moment, and its edges are the things a traveller can do:

1. **Ride edges** go from a train's departure at one stop to its arrival at a later stop of the same trip.
2. **Wait edges** go from one moment at a station to the next moment at that station.
3. **Change edges** go from a train's arrival at a station to a departure at least the minimum change time later.
4. **Walk edges** connect nearby stations and take the estimated walking time.
5. **Access edges** go from a source node to the stations near the departure place. The source node is the departure place at a moment in its departure window.
6. **Egress edges** go from the stations near the arrival place to a single sink node.

A journey is then a path from the source to the sink, and the number of ride edges on the path is the number of trains.

### Costs

To compare journeys, every edge gets a cost in minutes of train travel, and the cost of a journey is the sum over its edges. The terms are:

| Term | Edges it sits on | Weight |
|---|---|---|
| Time from leaving the place to reaching the next | every edge, by its duration | 1 |
| Extra weight of the way to and from a station | access, egress and final walk edges | 0.5 on top of the duration |
| Change penalty | change edges | 10 minutes, plus up to 110 by transfer aversion |
| Arriving before the arrival window | egress edges, by the arrival moment | 1 per minute before |
| Arriving after the arrival window | egress edges, by the arrival moment | 2 per minute after |
| Sleep in a berth between bedtime and wake-up | ride edges of trains with berths | −1 per minute of night covered, summed over all such trains |
| Night minutes not in a berth, until arrival | ride, wait and change edges during the night | 3 per minute |

How do these weights play out? At a middling transfer aversion, one change counts as 65 minutes of travel. A whole night of 7.5 hours without a berth counts as almost a day. Arriving early costs as much as the wait until the window opens, so an earlier arrival after the same departure never costs more than a later one.

With a price weight, the cost holds one more term. That term is the weight times the median of the estimated price of the journey's tickets and its night accommodation, in minutes of train travel. The traveller sets the weight as the value of an hour, and the planner stores it as the minutes per euro that follow from it. The presets are off, one minute per euro for a balanced traveller, three for a frugal one and eight for the cheapest.

Legs joined by a same-day stop share one more kind of edge. A stay edge goes from the arrival on one leg to the departure on the next, at the same place. It costs 2 per minute outside the stay range the traveller set. When the traveller marked the stop as "longer is better", the stay edge also costs −0.25 per minute beyond the minimum stay, up to the maximum.

### Standard form

Now we can write the problem down. Let $x_e \in \{0, 1\}$ say whether the journey uses edge $e$, let $c_e$ be the cost of the edge, and let $r_e$ be 1 on ride edges and 0 elsewhere. For a single leg the problem is

$$
\begin{aligned}
\min_{x} \quad & c^\top x \\
\text{subject to} \quad & N x = b, \\
& r^\top x \le K, \\
& x \in \{0, 1\}^{|E|},
\end{aligned}
$$

where $N$ is the node-edge incidence matrix of the time-expanded graph, and $b$ has a 1 at the source, a −1 at the sink and 0 everywhere else. $K$ is the largest number of trains, which is 5.

How hard is this problem? Without the ride constraint, it is a plain shortest path problem. Its constraint matrix is totally unimodular, so the linear relaxation already has an integral optimum, and label-setting algorithms such as Dijkstra's solve it directly. The ride constraint turns it into a resource-constrained shortest path problem, which is hard in general. Here it is easy in practice, because $K$ is small and the ride count only grows by one at a time. The search can therefore proceed round by round, one train at a time.

Every term sits on single edges, so the cost of a path is the sum of its edge costs. That holds even for the two terms that look global. The sleep term counts each train with berths for the night minutes it covers, and the arrival window term depends only on the arrival moment, which the egress edges carry.

With a price weight the problem has two objectives, the cost in time $c^\top x$ and the price $p(x)$. The planner minimizes their weighted sum $c^\top x + w \, p(x)$, which is the weighted sum scalarization of a bi-objective shortest path problem on a time-dependent graph. The price breaks the nice structure, though, because it is not a sum over edges. A ticket covers several rides, and its fare grows less than in proportion to the distance. The search therefore adds an estimate per ride, and the ranking afterwards prices the journeys it found from their tickets. Euler, Lindner and Borndörfer describe the exact version of this problem, with fares that depend on the rides before, as price-optimal routing on a conditional fare network.

### From legs to a trip

So far we looked at one leg. Legs that meet at a place with nights are independent, because the traveller sleeps between them, so each is a problem of its own. Legs joined by same-day stops form a chain. A chain is one shortest path problem over the layers of its legs, connected by the stay edges. The problem of the whole trip is the sum of these independent problems.

## Algorithmic structure

The planner does not solve the problem above in one go. Per leg, a search finds the journeys that no other journey beats on departure, arrival, number of trains and cost. A ranking then picks among them, with the costs of the rest of the chain added in. This is the a posteriori approach of multi-objective optimization. We first compute the set of solutions that no other solution beats on a few criteria, and then we pick among them with a weighted sum.

### Pareto search per leg

The search builds on RAPTOR, the round-based algorithm of Delling, Pajor and Werneck ("Round-Based Public Transit Routing", 2012). RAPTOR does not build the time-expanded graph, and it works on the timetable directly. In round $k$ it finds the earliest arrival at every station using at most $k$ trains, by scanning each route that serves a station improved in the round before. The rounds are exactly the ride constraint above, so the ride count comes for free.

Plain RAPTOR keeps one label per station and round, the earliest arrival. That is not enough for us, because a later arrival can have a lower cost. The planner therefore keeps a bag of labels instead, each with an arrival time and the cost added so far, as McRAPTOR from the same paper does. A label only drops another at the same station when it arrives no later and has cost no more.

Which costs does a label carry? The added cost is every term of the table except the time itself. That covers the extra weight of the ways to stations, the change penalties and the sleep on each ride. The time and the arrival window terms are added when a label reaches the arrival place, because they depend only on when the traveller left and arrived. Each term sits on single edges, so the cost adds up along the path as the standard form says.

The planner runs the search as a range query over the departure window, the variant known as rRAPTOR. It starts from the latest departure and works back, reusing the labels of later departures. The result is the set of journeys that no other journey beats on all four of a later departure, an earlier arrival, fewer trains and a lower cost. For a typical leg that is between 10 and 50 journeys. The planner splits the window into parts that run on separate threads and merges their results into one set.

Three techniques keep this fast.

- **Lower bounds.** A backward search from the arrival stations, over the fastest times between stations, gives a lower bound on the arrival for every station. With a lower bound on the cost still to come, the search drops a label when a journey already found arrives no later and costs no more than the label could at best. On a night leg the cost bound subtracts the sleep that is still possible. It caps that sleep by the berth trains the station can still reach, found in four tiers by how much of the night a train covers. That prunes labels that could not sleep anyway, but it stays weak where a full night train is within reach.
- **Service days.** The search skips the days on which no trip of a route runs.
- **Earlier trips.** On routes without berths, the cost of a ride does not depend on its times. An earlier trip that was boarded with no more cost therefore stands for the later trips of the same day.

MOTIS, whose routing core nigiri is also based on RAPTOR, uses similar techniques.

### Bounds from a search without prices

A search with prices keeps several times as many labels as one without, because journeys that are slow but cheap survive in it. Can we bound it with something cheap to compute? At a price weight above zero, the planner first runs a search without prices, and its journeys become the anchors. The planner prices each anchor at the traveller's weight. The cheapest anchor is a journey that the search with prices has to beat, so its cost, with a margin, becomes the cost limit of the search with prices. A label whose cost and lower bound already exceed the limit cannot lead to a journey worth showing. The search drops it from the first train on, and not only after it has found a good journey itself. The anchors then join the journeys of the search with prices, so the plan never loses one of them.

The cost limit also bounds the arrival. A journey costs at least the time from its departure to its arrival, less the most it can get back as credit for sleeping in a berth. A journey that leaves by the end of the window and arrives after some deadline therefore cannot stay within the limit. Before the search with prices, a second search runs backwards from the arrival place. Following the timetable round by round as RAPTOR does, it finds for every station the latest moment from which a traveller still reaches the arrival place by that deadline. The search with prices then drops every label that arrives at a station after that moment. The backward search keeps only moments that a label can reach at all. The earliest of these is the start of the window plus the fewest minutes from the departure place in the graph of fastest times between stations. Without that limit, the backward search would go back through the whole timetable before the window and cost more than it saves.

This is Bounded McRAPTOR from Delling, Dibbelt and Pajor. In terms of optimization it is branch and bound. The anchors give an upper bound on the best cost, the lower bounds of the search give a bound for each label, and a label goes when its bound exceeds the limit. The labels of each station form the branches, and the rounds of RAPTOR order the search.

Only the plan of a single leg at the traveller's weight uses anchors. The searches for the cheaper and the faster journey already have a cost limit, which is the cost of the chosen journey, so they get the backward search without anchors. The legs of a chain search without a limit, because the rest of the chain can make a costly journey the best one.

How much does this save? On the long leg from Stockholm to Palermo at 3 minutes per euro, the plan took 7.6 seconds without bounds, 3.7 seconds with the anchors, and 2.0 seconds with the backward search as well. On a typical leg the anchor search costs about as much as the bounds save, and the backward search adds a few milliseconds.

Other algorithms solve the same search problem. The Connection Scan Algorithm (Dibbelt et al., 2013) scans a sorted list of connections instead of routes, and Trip-Based Routing (Witt, 2015) precomputes the transfers between trips. RAPTOR fits well here for two reasons. The ride count is a natural part of it, and it works on a timetable that is rebuilt every few days without preprocessing.

### Ranking

The ranking computes each journey's cost from the table above and orders the journeys by it. The cheapest journey is the chosen one, unless the traveller picked another, and the alternatives are the next cheapest. The ranking leaves out an alternative that only repeats a shown journey at another time, with the same stations and train types. It also leaves out an alternative that costs more than twice the best.

Every journey of the plan also carries the estimated price of its tickets and its night accommodation. The price is the cheapest split of the journey's rides into tickets, each priced the way the Tickets screen prices it, plus the extra that a berth costs over a seat. It assumes one adult in second class, whose fare is the one the price model of `docs/price-model.md` estimates. A journey with no known operator carries no price, and the leg view shows none.

The ranking weighs that price at the traveller's price weight. Should a journey that is one euro cheaper by the estimate win over one that arrives earlier? Not if the estimate is off by more than one euro. A dead band therefore keeps the noise of the estimate out. A price within ten percent of the cheapest price of the leg counts as the cheapest itself, because the model is off by more than that, and journeys that close in price decide on their times. Without a weight the price changes no ranking. The cache key of a leg includes the price weight only when it is above zero, so a plan without a weight keeps the key it would have without prices.

Because the cost is a sum of named terms, the difference between two journeys splits exactly into those terms. The planner gives every journey its cost broken into these parts. For a leg of a chain it adds one more part, with everything the rest of the chain adds. These parts let the traveller see why a journey was chosen. The app shows the one or two parts that differ most from the best alternative, such as an earlier arrival that outweighs one more change. It shows the same for each alternative and for the best journey on a nearby day, and it says which journeys the search left out.

### Night legs

For a leg with a night train, the departure window is the 8 hours before bedtime, and the arrival window moves to the morning after. Arriving before the wake-up time counts as arriving before the window. The search itself needs nothing else, because the sleep is part of the cost it adds up. Take a traveller who stays on a night train to a later stop. That traveller arrives later but sleeps longer, and the search keeps both journeys, because neither beats the other on arrival and cost.

### Chains of legs

For a chain, the planner searches the first leg with its own windows. Each later leg gets a window derived from the arrivals of the leg before it. The window runs from an hour before the earliest arrival plus the shortest stay, to the latest arrival plus the longest stay. Why the hour? It lets a stay a little short of the minimum compete, with its penalty, against a later train that arrives much later at the next place. A departure window the traveller set, or the window of a night train, bounds it further.

The pairing across the chain is dynamic programming over the layered problem, in two passes. A forward pass gives each journey the cost of the best partial chain that ends with it. A backward pass gives each journey the cost of the best rest of the chain after it. Their sum is the cost of the best complete chain through the journey, and that sum ranks the journeys of every leg. The chosen journeys then lie on one cheapest chain, and the alternatives of each leg are ordered by how good a chain they allow.

### Nearby days

After the plan, the planner solves each chain again with every date moved a day earlier and a day later. When the best journey of a leg on the other day costs at least 90 minutes and 15 percent less, the planner mentions that day. This is a search over one more discrete choice, the date. The traveller makes that choice by hand, because moving a leg changes the nights around it.

### Cheaper and faster journeys

A traveller who plans by time never sees what a journey costs, and one who weighs prices never sees how fast a journey could be. After the plan of a leg, the planner therefore runs up to two more searches. One search, at the cheapest weight, finds a journey that saves at least 15 percent or 10 euro of the chosen journey's price and takes at most 90 minutes longer. The other, without a weight, finds a journey that saves at least 30 minutes and costs at most 25 percent more. A traveller at the cheapest weight gets no search for a cheaper journey, and one without a weight gets none for a faster journey.

These journeys join the alternatives of the leg, marked as cheaper or faster, so a leg has one list of alternatives. The list is ordered by cost at the traveller's weight, and the leg view shows how each alternative differs from the chosen journey in time and price. The planner computes the cost parts of a journey from another search at the traveller's weight, so that they compare with the others and the explanation can use them. The stations of these journeys join the station table of the plan, which the journeys of the extras index.

The leg can pick the cheaper journey only if it knows the journey, and that is not a given. A search at the traveller's weight drops a journey that another journey ties with or beats, and the cheaper journey of a tie is such a journey. When the traveller picks a journey that the search does not find, the planner searches at the other weights and adds the pick from there, so a pick of any listed alternative holds. These searches come before the plan of the leg, so the one at the cheapest weight takes the journeys without prices as anchors, as the plan does. A cheaper journey of the extras is the best one at that weight, so the bounded search finds it. Only a pick that the bounded search misses gets the search without bounds. On the leg from Stockholm to Palermo with a picked cheaper journey, the bounds take the plan from 7.9 to 3.4 seconds.

Sometimes the cheaper journey leaves and arrives when the chosen journey does, and costs the planner no more. It is then the same trip for less, so it becomes the chosen journey, and the old one becomes an alternative. The traveller's own pick stays. So does the chosen journey of a leg joined to others by same-day stops, which is the one on the best chain.

Both searches look only for what the leg view would show. The search at the cheapest weight drops journeys that take more than 90 minutes longer than the chosen one. Both drop journeys that cost more than the chosen journey does at their weight, because the chosen journey is one of their results. They keep only the best journey, so the cost limit of the search is the best cost found and not twice it. On the 80 legs of the benchmark, this takes the extras of a daytime leg without a weight from a total of 15.2 to 4.7 seconds, and those of a night leg from 29 to 14 seconds.

The extra searches take longer than the plan itself, so the planner delivers the plan of a leg first and the extra journeys later. The plan of a leg arrives in a `LegPlanned` event that says whether its extras are pending. The extras follow in a `LegExtrasPlanned` event once every leg has its plan. The extras hold the parts of the plan they change, and these replace the plan's own. Until they arrive, the leg row shows a spinner in the same style as the one that shows while the plan is made. A run that a newer run replaces keeps the plan it has, and the next run finds the legs that still wait for their extras and searches for them. The result store gets a leg only with its extras, and the plans of nearby days stay in memory without them.

### Pass days

The pass options come with the other extras, after the plan of the leg. A pass changes which journey of a leg is best. On a day the pass covers, an ICE costs its reservation and not its fare, so a fast train that loses on single tickets can win. Which days the pass covers is a choice about the whole trip, so the planner cannot pick the journeys first and add a pass afterwards.

At a price weight above zero, and when some pass takes a train of the bundle, the planner therefore searches each leg a second time with the prices of a pass day. A ride the pass takes costs what the pass charges on top, which is the seat reservation or the pass price of a night accommodation. A ride the pass does not take costs its fare, as a ticket of its own. This search also counts the travel days a journey uses. The ticket optimizer counts a ride on the local date it leaves on, so a journey uses as many days as it has distinct local departure dates among the rides the pass takes.

From that search the leg keeps its pass options. For each number of travel days from one to three and each first date, the option is the journey with the lowest cost plus weighted pass price. The solve of `btt-ticketing` then picks one option per leg, a pass and a window of validity, so that the total is lowest. A leg joined to others by same-day stops gets no pass options, because its departure window follows the leg before it.

When a run finishes, the app hands the solve each planned leg, with its date, the journey it takes on single tickets and its pass options. The solve returns scenarios, which are single tickets everywhere and the best one or two passes, sorted by their weighted total. The itinerary lists them above the legs and picks the lowest total. A leg that rides on a pass day in the picked scenario shows the journey of its pass option, and the map and the Tickets screen follow. A child or an infant gets single tickets only, because the scenarios do not model a child that travels free with an adult.

The scenarios leave out two rules of the ticket optimizer. A pass covers a ride in the country of residence only at the ends of the trip, and the pass prices of the planner ignore that. And two legs with a short stop between them can share one ticket, while the solve adds their prices. The Tickets screen applies both rules, so its total can differ from that of the scenario.

### Live prices of the candidates

A live price from a booking site takes two seconds, so the search and the ranking never ask for one. After a leg is planned at a price weight above zero, the app looks up the live prices of the chosen journey and the next two, one journey after the other in the background, when the trip allows live prices. The live price of a journey is its single tickets for one adult in second class, the same tickets the estimate prices, with the site's price in place of each estimate it has one for. The lookups go through the booking sites the app already uses, keep to their pauses, and teach the price model like any other lookup.

The leg view then puts the live prices into the costs of those journeys and ranks them again. When a live price moves another journey ahead of the chosen one, the leg shows that journey first and says which one the estimates had put first. Live prices skip the dead band of ten percent, because they carry no noise of the model. A journey the traveller picked stays chosen and only gets its live price. A leg on a pass day keeps its pass journey, because no site gives its price.

## Heuristics and limitations

The planner is exact for part of the cost and a heuristic for the rest. This section lists where it departs from the stated problem, and what each departure costs.

### Pruning in the search

The search is exact for the stated cost in all but three places.

1. **Only the earliest trip.** At each stop, a label boards the earliest trip of a route it can reach, as in RAPTOR. The search only tries a later trip of the same route from a later label. On a route with berths, though, a later trip can cover more of the night and cost less. A traveller who could wait for it at a station in the evening, but does not leave the place later, will not get it. That is rare, because the search also starts from every later departure from the place.
2. **Costly journeys.** Once the search has found a journey, it drops labels that cannot cost less than twice that journey's cost, even when they would leave later or arrive earlier. The ranking shows no alternative that costs more than twice the best, so these journeys would only matter for a chain, where the rest of the chain can make an otherwise costly journey the best.
3. **The margin of the anchors.** The cost limit from the anchors is a cost of the search, which prices ride by ride. The ranking prices a journey from its tickets, with the dead band of ten percent. A journey can therefore cost the search more than the limit and still rank first, when its tickets cost much less than its rides add up to. The margin of a tenth of the anchor's price cost plus two minutes covers most of these. On the 80 legs of the benchmark at 3 minutes per euro, two legs lose their best journey this way. Frankfurt to Rostock ranks first a journey that costs 3.5 percent more than the best, and Pratola Peligna to Esquelbecq one that costs 0.25 percent more. A margin of half the price cost still loses the first and makes the long legs slow again. In return, 21 legs get a better journey, because an anchor ranks above every journey of the search with prices. Two more legs choose a journey that costs a few minutes more, because the anchors shift the cheapest price that the dead band counts from.

Four shortcuts keep the number of scans down, and all four are exact.

- The first round of a run boards only the trains that leave when the traveller reaches the station. A train that leaves later belongs to the run for its own departure time, which leaves later and so beats this one.
- A trip that an earlier run or round boarded at the same stop or an earlier one, with no higher cost and no more rides, is not boarded again, because everything it reaches is in the bags already.
- A route that runs on none of the days a label could board is not queued.
- A scan of a route skips the stops that have no labels from the previous round until it rides a train.

At a price weight above zero, the search adds the price of every ride to the cost it accumulates. The journeys that survive are then good on the sum, and a cheap slow journey is not dropped for being slow. A table built from the posterior of the price model gives the price of a ride by a lookup, as `docs/price-model.md` describes. The two boarding shortcuts then count the price of the stops between the boardings, because boarding earlier pays for the extra distance. That price bounds the extra only while both boardings are priced by the same class of ride. A route that crosses a border changes class, and a ride that boards before the border pays its own rate for the whole distance. For a pair of boardings in two classes the shortcuts therefore make no claim, and the search keeps both. A ride that follows another of the same operator pays the extra over the ride before it, so the search overcounts a ticket run by no more than the base fare of the extra rides. Without a weight the price touches nothing, and the search is the search without prices.

On pass days the travel days are one more criterion, and every shortcut has to respect it. A journey on fewer days must stay beside a cheaper one on more, because the solve may have no pass day left for the cheaper one. A label covers another only when it uses fewer days, or the same number with the same last day. Why is the same count with another last day not enough? A ride on the last day of one label would add a day to the other label only. The two boarding shortcuts compare the days after the ride in the same way. Arrivals at the arrival place, and the cost limit they set, prune only labels with at least as many days. The search drops a journey that would use more than three days, since no pass option holds it.

A train arrival that another train arrival covers does not walk on to the stations next to it. The label that covers it has walked on, or will, and its walks cover the walks from here. A label that arrived on foot cannot walk on, so an arrival that only such labels cover still does. Nearly nine in ten labels of a priced search are covered when they are made, so this skips most of the footpath work, about a tenth of the CPU time of a priced search.

The walks from a station go shortest first. Once a walk arrives too late or costs too much to beat a journey found so far, at any station, every longer walk does too, so the search stops there. This is the early pruning of Rohovyi, Abuaisha and Walsh. It leaves out the lower bound of the station the walk ends at, because that bound differs per station and would not hold for the longer walks. On a night leg a later arrival can cost less, through the credit for sleep, so the walks of a night leg all go through the usual test.

The backward search of the anchors is exact. A property test compares its latest moments with the forward search on random timetables. A label that arrives at a station after its latest moment cannot reach the arrival place by the deadline.

A label from the departure place does not stand for a journey, even at a station where the arrival place also has a candidate. It also does not keep a train that arrives there from standing for one. Without this rule, a leg between two places that share a station could miss journeys, because the walk would cover the train arrival in the bag.

The search counts cost in hundredths of a minute, while the ranking rounds the extra weight of the ways to stations to whole minutes once per journey. The two differ by less than a minute, and the ranking recomputes the cost of every journey the search returns.

### Speed of the search

Searching with the cost as a criterion takes longer than the plain search on arrival and trains, because each station holds more labels and the cost bound prunes less. The benchmark plans the MOTIS benchmark queries as legs, on a bundle of 1.2 million trips. The median planning time is 33 milliseconds for daytime legs and 207 milliseconds for night legs, and the slowest leg takes 1.6 seconds. The dense city networks gain most from the shortcuts above, and Stockholm to Uppsala goes from 0.87 to 0.12 seconds. What does the cost criterion buy? Of the 500 night legs, 187 get a cheaper journey than the plain search finds, and so do 21 of the 500 daytime legs. None gets a costlier one. Most of the night gains come from night trains that a seated train arriving earlier pushes out of the results of the plain search.

Prices make the search slower still, because each station keeps more labels. Journeys that a search by time drops for being slow survive when they are cheap. On the first 40 benchmark queries, which are 80 legs, a search at a weight of 3 minutes per euro makes 3.7 times as many labels as a search without a weight and takes about 3 times as long.

Not every search of a plan blocks the leg. The plan of the leg at its weight comes first. The searches for the cheaper journey, the faster journey and the journeys on pass days follow as the extras of the leg. The table shows the time of both in milliseconds, as the median and the 90th percentile of those 80 legs. For comparison, the first row is a planner without any price terms.

| Weight | Plan, daytime | Plan, night | Extras, daytime | Extras, night |
|---|---|---|---|---|
| Planner without prices | 22 / 170 | 232 / 434 | none | none |
| None | 27 / 173 | 247 / 514 | 28 / 169 | 77 / 687 |
| 1 minute per euro | 44 / 393 | 551 / 1,280 | 67 / 467 | 539 / 1,113 |
| 3 minutes per euro | 41 / 548 | 568 / 1,263 | 68 / 513 | 500 / 1,051 |
| 8 minutes per euro | 44 / 787 | 515 / 1,400 | 38 / 376 | 415 / 874 |

On all 1000 legs of the benchmark, the plan without a weight takes a median of 32 ms for a daytime leg and 239 ms for a night leg, against 31 and 232 ms for the planner without prices. The chosen journey costs the same in both for all 1000 legs. Six night legs pick other trains at the same cost, because the walks from a station go shortest first and a tie can then go the other way.

Two techniques make the priced search faster. The boarding shortcuts ask the model for the price of the stops between two boardings only after every cheaper condition has held, which saves most of those calls. And the search reads prices from a curve per class and not from the formula. Together they cut the median planning time by about a third, and the 90th percentile by up to half. What remains comes from the number of labels.

Could a better lower bound prune more labels? A bound that adds the price of the distance still to cover, at the lowest price per kilometre of any class, would seem to. It does nothing here, though. The fitted price of 10 of the 1471 classes in the benchmark bundle is flat over some distance beyond 20 km. The lowest rate is therefore zero, and the bound adds nothing.

### The sleep term

The sleep term sums the night minutes over every train with berths. A journey that changes between two night trains in the middle of the night gets the sleep on both counted in full, while in reality the change wakes the traveller up. Such journeys are rare, and the minutes of the change itself still count as awake.

### Chains

The planner searches each later leg of a chain from an hour before the shortest stay ends. A stay more than an hour short of the minimum is never searched, even when the penalty would allow it. A stay without a maximum ends with the leg's own departure window. The dynamic programming across the chain is exact for the journeys the searches found, so the searches themselves are the limit.

### Walking and stations

The ways between a place and a station, and between nearby stations, are estimates from the straight-line distance. Below one kilometre the planner assumes walking at 5 km/h with a detour factor of 1.3. Above that it assumes local transport, at 15 minutes plus the distance at 15 km/h. The planner does not route on streets or local networks. The search considers the 16 closest stations within the search radius of the place. Walks between stations take one hop, so a walk from one station through a second to a third only exists when the first and the third are close enough on their own.

### The timetable

The costs are only as good as the timetable. The builder matches stations from different feeds by code, by name and distance, and by distance alone, and it merges copies of the same train in different feeds. A mistake in either step shows up as a missing train, as a change that is not possible in reality, or as a change that the traveller does not need. A station that the Trainline dataset lists as part of another only joins it when the two lie within one kilometre. The dataset also files separate stations under a main station, such as an airport station under the main station of the city, and the distance check keeps those apart.

Each ride in a plan names its operator, from the agency of its trip in the bundle. The feeds name one operator in many ways, so `data/operators.toml` maps the agency names, and for some brands the category too, to one name per operator. For example, `regiojet` stands for "RegioJet a.s." and "RegioJet,a.s.", and `ouigo` for an OUIGO train of an SNCF agency. The operator plays no part in the costs. It decides where a ticket is bought, whether a pass covers the train, and which prices the ticket price model of `docs/price-model.md` estimates for it. When the builder merges copies of a train from several feeds, the agency of the copy it keeps wins, so a train run by two operators together can carry either name.

The feeds do not end on the same day. Most national feeds stop at the yearly timetable change in December, while the bundle covers up to 128 days from the day it is built. The planner plans a leg on any day of the bundle's window and refuses a leg outside it. Inside the window, a day after the end of a feed has no trains of that feed's operators, and the plan then shows only what the other feeds offer. The app finds each operator's last day from the last service day of its trains, once per bundle. It lists the operators in `data/operators.toml` whose last day is before the end of the window, and warns the traveller about those whose trains end before the day a trip reaches its destination. An operator that the file does not list has no name to show, so the app does not warn about it.

A train label of the bundle can carry a link to a page on vagonWEB that shows the layout of the train, the order of its coaches. The address of a train page follows from the operator, category, number and year of the train, but the numbers that have a page follow no rule, and the site answers a train without a page with an empty page and the status 200. So the builder links only to the trains in the dictionary `data/vagonweb-trains.txt`, which lists every train page of the current timetable year. The example `vagonweb_dictionary` of `btt-builder` writes the file from the two sitemaps of the site, and the build reads the file and makes no request to the site. The site refuses requests from the servers of GitHub, so the example has to run on a normal connection, and the file has to be written again and committed when the site publishes the pages of a new timetable year. The file `data/vagonweb.toml` maps the agency of a train, and for some brands its category, to the operator code and the category that the site uses. A label gets the page of the train itself when the dictionary lists it. For categories that keep their number in every country, such as `ICE` and `EC`, it can also get the page of another operator's train, which the app marks as approximate. Otherwise it gets the list of the category and then the list of the operator. A label that no rule matches gets no link. The dictionary only holds the pages of one timetable year, which starts on the second Sunday of December, so the planner adds the link to a ride only when the date of the ride lies in the year of the page. The links play no part in the costs. The build report lists the matches by category, which shows a category that needs a rule.

The minimum time for a change is the traveller's trip-wide setting at every station, unless the traveller set a time for a single leg. A leg with its own time uses it for every change on that leg, and the other legs keep the trip-wide time. Legs joined by a same-day stop are still searched together, but each leg applies its own minimum. The planner does not follow feeds that give a longer time for particular stations or trains, so a change that the setting allows can be too short in reality. This matters most for trains with check-in, such as Eurostar, which closes boarding well before departure.

### Comparison with MOTIS

MOTIS, run on the same rail feeds with its own benchmark queries, gives a check on both the search and the timetable. Of 500 random queries between stops, 363 have the same earliest arrival. In 84 more, the planner arrives earlier by walking or by taking local transport between stations up to five kilometres apart, which MOTIS without street data does not link. Most of the remaining differences are a few minutes and come from the minimum change time above. The rest come from trains that only one of the two finds in the feeds.

## References

- D. Delling, T. Pajor, R. F. Werneck. Round-Based Public Transit Routing. ALENEX 2012, and Transportation Science 49(3), 2015. Describes RAPTOR, rRAPTOR and McRAPTOR.
- J. Dibbelt, T. Pajor, B. Strasser, D. Wagner. Intriguingly Simple and Fast Transit Routing. SEA 2013. Describes the Connection Scan Algorithm.
- S. Witt. Trip-Based Public Transit Routing. ESA 2015.
- S. Irnich, G. Desaulniers. Shortest Path Problems with Resource Constraints. In Column Generation, Springer, 2005.
- D. Delling, J. Dibbelt, T. Pajor. Fast and Exact Public Transit Routing with Restricted Pareto Sets. ALENEX 2019. Describes Bounded McRAPTOR, with anchor journeys and a backward pruning search.
- A. Rohovyi, T. Abuaisha, T. Walsh. Early Pruning for Public Transport Routing. 2026. Sorts the footpaths of a station and stops at the first one that cannot improve a journey.
- M. Müller-Hannemann, M. Schnee. Finding All Attractive Train Connections by Multi-Criteria Pareto Search. In Algorithmic Methods for Railway Optimization, Springer, 2007. Searches the Deutsche Bahn timetable with fares as a criterion and a relaxed dominance.
- R. Euler, N. Lindner, R. Borndörfer. Price Optimal Routing in Public Transportation. EURO Journal on Transportation and Logistics, 2024. States the exact problem with fares that depend on the rides before, as a shortest path on a conditional fare network.
- M. Ehrgott. Multicriteria Optimization. Springer, 2005. Covers the weighted sum scalarization and the a posteriori approach.
- A. H. Land, A. G. Doig. An Automatic Method of Solving Discrete Programming Problems. Econometrica 28(3), 1960. The origin of branch and bound.
- L. Mandow, J. L. Pérez de la Cruz. Multiobjective A* Search with Consistent Heuristics. Journal of the ACM 57(5), 2010. Describes NAMOA*, a label-setting search for several objectives with lower bounds on each.
- MOTIS and its routing core nigiri, at [github.com/motis-project](https://github.com/motis-project).
