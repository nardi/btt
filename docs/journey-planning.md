# Journey planning in BTT

This document describes how BTT picks the trains for a trip. It first states the problem as an optimization problem, then describes how the planner solves it and how that relates to existing work on public transit routing, and it ends with the places where the solution method departs from the stated problem.

## The problem

A trip is a list of places. Between two consecutive places lies a leg, and each place in the middle has a stay: either a number of nights, or a same-day stop of a few hours. The traveller sets time windows for leaving and arriving, a transfer aversion, a minimum time for changing trains, and for a leg with a night train a bedtime and a wake-up time. The planner has to pick one journey per leg: a sequence of train rides with the changes and walks between them.

### Time-expanded graph

We can write a leg as a shortest path problem on a time-expanded graph. Its nodes are pairs of a station and a moment, and its edges are the things a traveller can do:

1. **Ride edges** from a train's departure at one stop to its arrival at a later stop of the same trip.
2. **Wait edges** from one moment at a station to the next moment at that station.
3. **Change edges** from a train's arrival at a station to a departure at least the minimum change time later.
4. **Walk edges** between nearby stations, taking the estimated walking time.
5. **Access edges** from a source node, the departure place at a moment in its departure window, to the stations near that place.
6. **Egress edges** from the stations near the arrival place to a single sink node.

A journey is then a path from the source to the sink, and the number of rides on the path is the number of trains.

### Costs

Every edge gets a cost in minutes of train travel, and the cost of a journey is the sum over its edges. The terms are:

| Term | Edges it sits on | Weight |
|---|---|---|
| Time from leaving the place to reaching the next | every edge, by its duration | 1 |
| Extra weight of the way to and from a station | access, egress and final walk edges | 0.5 on top of the duration |
| Change penalty | change edges | 10 minutes, plus up to 110 by transfer aversion |
| Arriving before the arrival window | egress edges, by the arrival moment | 1 per minute before |
| Arriving after the arrival window | egress edges, by the arrival moment | 2 per minute after |
| Sleep in a berth between bedtime and wake-up | ride edges of trains with berths | −1 per minute of night covered, summed over all such trains |
| Night minutes not in a berth, until arrival | ride, wait and change edges during the night | 3 per minute |

With these weights, one change at a middling transfer aversion counts as 65 minutes of travel, and a whole night of 7.5 hours without a berth counts as almost a day. Arriving early costs as much as the wait until the window opens, so an earlier arrival after the same departure never costs more than a later one.

With a price weight the cost holds one more term: the weight times the median of the estimated price of the journey's tickets and its night accommodation, in minutes of train travel. The weight is the value the traveller gives to an hour, stored as the minutes per euro that follow from it. The presets are off, one minute per euro for a balanced traveller, three for a frugal one and eight for the cheapest.

Legs joined by a same-day stop share one more kind of edge: a stay edge from the arrival on one leg to the departure on the next, at the same place. Its cost is 2 per minute outside the stay range the traveller set. When the traveller marked the stop "longer is better", it is also −0.25 per minute beyond the minimum stay, up to the maximum.

### Standard form

Write $x_e \in \{0, 1\}$ for whether the journey uses edge $e$, $c_e$ for the edge's cost, and $r_e$ for 1 on ride edges and 0 elsewhere. For a single leg the problem is

$$
\begin{aligned}
\min_{x} \quad & c^\top x \\
\text{subject to} \quad & N x = b, \\
& r^\top x \le K, \\
& x \in \{0, 1\}^{|E|},
\end{aligned}
$$

where $N$ is the node-edge incidence matrix of the time-expanded graph, $b$ has a 1 at the source, a −1 at the sink and 0 everywhere else, and $K$ is the largest number of trains, currently 5.

Without the ride constraint, this is a plain shortest path problem. Its constraint matrix is totally unimodular, so the linear relaxation already has an integral optimum, and label-setting algorithms such as Dijkstra's solve it directly. The ride constraint turns it into a resource-constrained shortest path problem, which is hard in general. With a small $K$ and ride counts that only grow by one at a time, it is easy in practice, because the search can proceed round by round, one train at a time.

Every term sits on single edges, so the cost of a path is the sum of its edge costs. The sleep term counts each train with berths for the night minutes it covers, and the arrival window term depends only on the arrival moment, which the egress edges carry.

### From legs to a trip

Legs that meet at a place with nights are independent, because the traveller sleeps between them, so each is its own problem. Legs joined by same-day stops form a chain, and the chain is one shortest path problem over the layers of its legs, connected by the stay edges. The trip's problem is the sum of these independent problems.

## Algorithmic structure

The planner doesn't solve the problem above in one go. Per leg, a search finds the journeys that no other journey beats on departure, arrival, number of trains and cost. A ranking then picks among them, with the costs of the rest of the chain added in. This is the a posteriori approach of multi-objective optimization: first compute the set of solutions that no other solution beats on a few criteria, then pick among them with a weighted sum.

### Pareto search per leg

The search builds on RAPTOR, the round-based algorithm of Delling, Pajor and Werneck ("Round-Based Public Transit Routing", 2012). RAPTOR doesn't build the time-expanded graph. It works on the timetable directly: round $k$ finds the earliest arrival at every station using at most $k$ trains, by scanning each route that serves a station improved in the round before. The rounds are exactly the ride constraint above, so the ride count comes for free.

Plain RAPTOR keeps one label per station and round, the earliest arrival. The planner keeps a bag of labels instead, each with an arrival time and the cost added so far, as McRAPTOR from the same paper does. A label only drops another at the same station when it arrives no later and has cost no more. The added cost is every term of the table except the time itself: the extra weight of the ways to stations, the change penalties and the sleep on each ride. The time and the arrival window terms are added when a label reaches the arrival place, because they depend only on when the traveller left and arrived. Each term sits on single edges, so the cost adds up along the path as the standard form says.

The planner runs the search as a range query over the departure window, the variant known as rRAPTOR. It starts from the latest departure and works back, reusing the labels of later departures. The result is the set of journeys that no other journey beats on all four of: a later departure, an earlier arrival, fewer trains and a lower cost. For a typical leg that is between 10 and 50 journeys. The window is split into parts that run on separate threads, and the results are merged into one set.

Three techniques keep this fast. A backward search from the arrival stations over the fastest times between stations gives a lower bound on the arrival for every station. With a lower bound on the cost still to come, a label is dropped when a journey already found arrives no later and costs no more than the label could at best. On a night leg the cost bound subtracts the sleep that is still possible. It caps that sleep by the berth trains the station can still reach, found in four tiers by how much of the night a train covers, which prunes labels that could not sleep anyway but stays weak where a full night train is within reach. The search also skips the days on which no trip of a route runs. And on routes without berths, where a ride's cost doesn't depend on its times, an earlier trip that was boarded with no more cost stands for the later trips of the same day. MOTIS, whose routing core nigiri is also based on RAPTOR, uses similar techniques.

Other algorithms solve the same search problem. The Connection Scan Algorithm (Dibbelt et al., 2013) scans a sorted list of connections instead of routes, and Trip-Based Routing (Witt, 2015) precomputes the transfers between trips. RAPTOR fits well here because the ride count is a natural part of it, and because it works on a timetable that is rebuilt every few days without preprocessing.

### Ranking

The ranking computes each journey's cost from the table above and orders the journeys by it. The cheapest journey is the chosen one, unless the traveller picked another, and the alternatives are the next cheapest. An alternative that only repeats a shown journey at another time, with the same stations and train types, is left out, and so is one that costs more than twice the best.

Every journey of the plan also carries the estimated price of its tickets and its night accommodation. The price is the cheapest split of the journey's rides into tickets, each priced the way the Tickets screen prices it, plus the extra a berth costs over a seat. It assumes one adult in second class, whose fare is the one the price model of `docs/price-model.md` estimates. A journey with no known operator carries no price, and the leg view shows none.

The ranking weighs that price at the traveller's price weight. A dead band keeps the noise of the estimate out: a price within ten percent of the cheapest price of the leg counts as the cheapest itself, because the model is off by more than that, so journeys that close in price decide on their times. Without a weight the price changes no ranking, and the cached results of such plans keep the keys they always had.

Because the cost is a sum of named terms, the difference between two journeys splits exactly into those terms. The planner gives every journey its cost broken into these parts, and for a leg of a chain one more part with everything the rest of the chain adds. From them, the traveller can see why a journey was chosen: the one or two parts that differ most from the best alternative, such as an earlier arrival that outweighs one more change, the same for each alternative and for the best journey on a nearby day, and which journeys the search left out.

### Night legs

For a leg with a night train, the departure window is the 8 hours before bedtime, and the arrival window moves to the morning after. Arriving before the wake-up time counts as arriving before the window. The search itself needs nothing else, because the sleep is part of the cost it adds up. A traveller who stays on a night train to a later stop arrives later but sleeps longer, and the search keeps both journeys because neither beats the other on arrival and cost.

### Chains of legs

For a chain, the first leg is searched with its own windows. Each later leg is searched in a window derived from the arrivals of the leg before it: from an hour before the earliest arrival plus the shortest stay, to the latest arrival plus the longest stay. The hour lets a stay a little short of the minimum compete, with its penalty, against a later train that arrives much later at the next place. A departure window the traveller set, or a night train's window, bounds it further.

The pairing across the chain is dynamic programming over the layered problem, in two passes. A forward pass gives each journey the cost of the best partial chain ending with it, and a backward pass gives the cost of the best rest of the chain after it. Their sum is the cost of the best complete chain through the journey, which ranks the journeys of every leg. The chosen journeys then lie on one cheapest chain, and the alternatives of each leg are ordered by how good a chain they allow.

### Nearby days

After the plan, the planner solves each chain again with every date moved a day earlier and a day later. When a leg's best journey on the other day costs at least 90 minutes and 15% less on its own, the planner mentions that day. This is a search over one more discrete choice, the date, which the traveller makes by hand because moving a leg changes the nights around it.

### Cheaper and faster journeys

A traveller who plans by time never sees what a journey costs, and one who weighs prices never sees how fast a journey could be. After the first search of a leg, the planner therefore runs up to two more: one at the cheapest weight, whose best journey shows as the cheaper option when it saves at least 15 percent or 10 euro of the chosen journey's price and takes at most 90 minutes longer, and one without a weight, whose best journey shows as the faster option when it saves at least 30 minutes and costs at most 25 percent more. A traveller at the cheapest weight sees no cheaper option, and one without a weight sees no faster one. The extra searches run after the first, so the chosen journey is decided before any more work happens, and a leg that stops early keeps the plan it has.

### Pass days

A pass changes which journey of a leg is best. On a day the pass covers, an ICE costs its reservation and not its fare, so a fast train that loses on single tickets can win. Which days the pass covers is a choice of the whole trip, so the planner can't pick the journeys first and add a pass afterwards.

At a price weight above zero, when some pass takes a train of the bundle, the planner searches each leg a second time with the prices of a pass day. A ride the pass takes costs what the pass charges on top, which is the seat reservation or the pass price of a night accommodation. A ride the pass does not take costs its fare, as a ticket of its own. This search also counts the travel days a journey uses. The ticket optimizer counts a ride on the local date it leaves on, so a journey uses as many days as it has distinct local departure dates among the rides the pass takes.

From that search the leg keeps its pass options. For each number of travel days from one to three and each first date, the option is the journey with the lowest cost plus weighted pass price. The solve of `btt-ticketing` then picks one option per leg, a pass and a window of validity, so that the total is lowest. A leg joined to others by same-day stops gets no pass options, because its departure window follows the leg before it.

When a run finishes, the app hands the solve each planned leg: its date, the journey it takes on single tickets and its pass options. The solve returns scenarios, which are single tickets everywhere and the best one or two passes, sorted by their weighted total. The itinerary lists them above the legs and picks the lowest total. A leg that rides on a pass day in the picked scenario shows the journey of its pass option, and the map and the Tickets screen follow. A child or an infant gets single tickets only, because the scenarios do not model a child that travels free with an adult.

The scenarios leave out two rules of the ticket optimizer. A pass covers a ride in the country of residence only at the ends of the trip, which the pass prices of the planner ignore. And two legs with a short stop between them can share one ticket, while the solve adds their prices. The Tickets screen applies both rules, so its total can differ from the scenario's.

## Heuristics and limitations

The planner is exact for part of the cost and a heuristic for the rest. This section lists where it departs from the stated problem.

### Pruning in the search

The search is exact for the stated cost in all but two places.

1. **Only the earliest trip.** At each stop, a label boards the earliest trip of a route it can reach, as in RAPTOR. A later trip of the same route is only tried from a later label. On a route with berths, a later trip can cover more of the night and cost less. A traveller who could wait for it at a station in the evening but doesn't leave the place later won't get it, which is rare because the search also starts from every later departure from the place.
2. **Costly journeys.** Once a journey is found, labels that can't cost less than twice its cost are dropped, even when their journeys would leave later or arrive earlier. The ranking shows no alternative that costs more than twice the best, so these journeys would only matter for a chain, where the rest of the chain can make an otherwise costly journey the best.


Four exact shortcuts keep the number of scans down. The first round of a run boards only the trains that leave when the traveller reaches the station, because a train that leaves later belongs to the run for its own departure time, which leaves later and so beats this one. A trip that an earlier run or round boarded at the same stop or an earlier one, with no higher cost and no more rides, is not boarded again, because everything it reaches is in the bags already. A route that runs on none of the days a label could board is not queued, and a scan of a route skips the stops that have no labels from the previous round until it rides a train.

At a price weight above zero, the search adds the price of every ride to the cost it accumulates, so the journeys that survive are good on the sum and a cheap slow journey is not dropped for being slow. A table built from the price model's posterior prices a ride in a few multiplications and one exponential, as `docs/price-model.md` describes. The two boarding shortcuts then count the price of the stops between the boardings, because boarding earlier pays for the extra distance. A ride that follows another of the same operator pays the extra over the ride before it, so the search overcounts a ticket run by no more than the base fare of the extra rides. Without a weight the price touches nothing and the search runs exactly as before.

On pass days the travel days are one more criterion, and every shortcut has to respect it. A journey on fewer days must stay beside a cheaper one on more, because the solve may have no pass day left for the cheaper one. A label covers another only when it uses fewer days, or the same number with the same last day. The same count with another last day is not enough, because a ride on the other's last day would add a day to one of them only. The two boarding shortcuts compare the days after the ride in the same way. Arrivals at the arrival place, and the cost limit they set, prune only labels with at least as many days. A journey that would use more than three days is dropped, since no pass option holds it.

A label from the departure place does not stand for a journey, even at a station where the arrival place also has a candidate, and does not keep a train that arrives there from standing for one. Without this rule, a leg between two places that share a station could miss journeys, because the walk covered the train arrival in the bag.
The search counts cost in hundredths of a minute, while the ranking rounds the extra weight of the ways to stations to whole minutes once per journey. The two differ by less than a minute, and the ranking recomputes the cost of every journey the search returns.

Searching with the cost as a criterion takes longer than the plain search on arrival and trains, because each station holds more labels and the cost bound prunes less. On the MOTIS benchmark queries planned as legs, on a bundle of 1.2 million trips, the median planning time is 33 milliseconds for daytime legs and 207 milliseconds for night legs, and the slowest leg takes 1.6 seconds. The dense city networks gain most from the shortcuts above, with Stockholm to Uppsala going from 0.87 to 0.12 seconds. In return, 187 of the 500 night legs and 21 of the 500 daytime legs get a cheaper journey, and none gets a costlier one. Most of the night gains come from night trains that a seated train arriving earlier had pushed out of the plain search's results.

### The sleep term

The sleep term sums the night minutes over every train with berths. A journey that changes between two night trains in the middle of the night gets the sleep on both counted in full, while in reality the change wakes the traveller up. Such journeys are rare, and the minutes of the change itself still count as awake.

### Chains

Each later leg of a chain is searched from an hour before the shortest stay ends. A stay more than an hour short of the minimum is never searched, even when the penalty would allow it, and a stay without a maximum ends with the leg's own departure window. The dynamic programming across the chain is exact for the journeys the searches found, but the searches themselves are the limit.

### Walking and stations

The ways between a place and a station, and between nearby stations, are estimates from the straight-line distance: walking at 5 km/h with a detour factor of 1.3 below one kilometre, and above that 15 minutes plus the distance at 15 km/h for local transport. The planner doesn't route on streets or local networks. The search considers the 16 closest stations within the place's search radius, and walks between stations take one hop, so a walk from one station through a second to a third only exists when the first and the third are close enough on their own.

### The timetable

The costs are only as good as the timetable. Stations from different feeds are matched by code, by name and distance, and by distance alone, and copies of the same train in different feeds are merged. A mistake in either step shows up as a train that is missing, a change that isn't possible in reality, or a change that the traveller doesn't need. A station that the Trainline dataset lists as part of another only joins it when the two lie within one kilometre, because the dataset also files separate stations under a main station, such as an airport station under the city's main station.

Each ride in a plan names its operator, from the agency of its trip in the bundle. The feeds name one operator in many ways, so `data/operators.toml` maps the agency names, and for some brands the category too, to one name per operator, such as `regiojet` for "RegioJet a.s." and "RegioJet,a.s.", or `ouigo` for an OUIGO train of an SNCF agency. The operator plays no part in the costs. It decides where a ticket is bought, whether a pass covers the train and which prices the ticket price model of `docs/price-model.md` estimates for it. When copies of a train from several feeds are merged, the agency of the copy that is kept wins, so a train run by two operators together can carry either name.

The minimum time for a change is the traveller's setting at every station. Feeds that give a longer time for particular stations or trains are not followed, so a change that the setting allows can be too short in reality. This matters most for trains with check-in, such as Eurostar, which closes boarding well before departure.

### Comparison with MOTIS

MOTIS, run on the same rail feeds with its own benchmark queries, gives a check on both the search and the timetable. Of 500 random queries between stops, 363 have the same earliest arrival. In 84 more, the planner arrives earlier by walking or taking local transport between stations up to five kilometres apart, which MOTIS without street data doesn't link. Of the remaining differences, most are a few minutes and come from the minimum change time above, and the rest from trains that only one of the two finds in the feeds.

## References

- D. Delling, T. Pajor, R. F. Werneck. Round-Based Public Transit Routing. ALENEX 2012, and Transportation Science 49(3), 2015. Describes RAPTOR, rRAPTOR and McRAPTOR.
- J. Dibbelt, T. Pajor, B. Strasser, D. Wagner. Intriguingly Simple and Fast Transit Routing. SEA 2013. Describes the Connection Scan Algorithm.
- S. Witt. Trip-Based Public Transit Routing. ESA 2015.
- S. Irnich, G. Desaulniers. Shortest Path Problems with Resource Constraints. In Column Generation, Springer, 2005.
- MOTIS and its routing core nigiri, at [github.com/motis-project](https://github.com/motis-project).
