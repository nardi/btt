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
| Arriving outside the arrival window | egress edges, by the arrival moment | 2 per minute outside |
| Sleep in a berth between bedtime and wake-up | ride edges of trains with berths | −1 per minute of night covered |
| Night minutes not in a berth, until arrival | ride, wait and change edges during the night | 3 per minute |

With these weights, one change at a middling transfer aversion counts as 65 minutes of travel, and a whole night of 7.5 hours without a berth counts as almost a day.

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

Two terms don't fit this form exactly. The sleep term counts only the single train with berths that covers most of the night, not the sum over all trains with berths. That is a maximum over rides instead of a sum. And the arrival window term depends on the arrival moment, which the egress edges carry, but it makes the cost not grow with arrival time: arriving too early costs more than arriving just in time.

### From legs to a trip

Legs that meet at a place with nights are independent, because the traveller sleeps between them, so each is its own problem. Legs joined by same-day stops form a chain, and the chain is one shortest path problem over the layers of its legs, connected by the stay edges. The trip's problem is the sum of these independent problems.

## Algorithmic structure

The planner doesn't solve the problem above directly. It splits it into a search that doesn't know the costs, followed by a ranking that does. This is the a posteriori approach of multi-objective optimization: first compute the set of solutions that no other solution beats on a few criteria, then pick among them with a weighted sum.

### Pareto search per leg

The search is RAPTOR, the round-based algorithm of Delling, Pajor and Werneck ("Round-Based Public Transit Routing", 2012). RAPTOR doesn't build the time-expanded graph. It works on the timetable directly: round $k$ finds the earliest arrival at every station using at most $k$ trains, by scanning each route that serves a station improved in the round before. The rounds are exactly the ride constraint above, so the ride count comes for free.

The planner runs RAPTOR as a range query over the departure window, the variant known as rRAPTOR. It starts from the latest departure and works back, reusing the labels of later departures. The result is the set of journeys that no other journey beats on all three of: a later departure, an earlier arrival and fewer trains. For a typical leg that is between 10 and 30 journeys. The window is split into parts that run on separate threads, and the results are merged into one set.

Two techniques from the literature keep this fast. A backward search from the arrival stations over the fastest times between stations gives a lower bound for every station, and a label that can't beat the best arrival found so far, even with that bound, is dropped. And the search skips the days on which no trip of a route runs. Both are exact: they never drop a journey the plain search would keep. MOTIS, whose routing core nigiri is also based on RAPTOR, uses similar techniques.

Other algorithms solve the same search problem. The Connection Scan Algorithm (Dibbelt et al., 2013) scans a sorted list of connections instead of routes, and Trip-Based Routing (Witt, 2015) precomputes the transfers between trips. RAPTOR fits well here because the ride count is a natural part of it, and because it works on a timetable that is rebuilt every few days without preprocessing.

### Ranking

The ranking computes each journey's cost from the table above and orders the journeys by it. The cheapest journey is the chosen one, unless the traveller picked another, and the alternatives are the next cheapest. An alternative that only repeats a shown journey at another time, with the same stations and train types, is left out, and so is one that costs more than twice the best.

Because the cost is a sum of named terms, the difference between two journeys splits exactly into those terms. The planner can explain a choice by the terms that differ most, such as an earlier arrival that outweighs one more change.

### Night legs

For a leg with a night train, the departure window is the 8 hours before bedtime, and the arrival window moves to the morning after. Next to the normal search, a second search runs with two extra constraints:

1. Trains without berths can't be boarded between bedtime and 3 hours before the wake-up time.
2. A train with berths can't be left before the wake-up time, except at its last stop.

The journeys of both searches are united without pruning, and the ranking weighs the sleep against the travel time. The section on heuristics explains why the second search is needed.

### Chains of legs

For a chain, the first leg is searched with its own windows. Each later leg is searched in a window derived from the arrivals of the leg before it: from the earliest arrival plus the shortest stay to the latest arrival plus the longest stay. A departure window the traveller set, or a night train's window, bounds it further.

The pairing across the chain is dynamic programming over the layered problem, in two passes. A forward pass gives each journey the cost of the best partial chain ending with it, and a backward pass gives the cost of the best rest of the chain after it. Their sum is the cost of the best complete chain through the journey, which ranks the journeys of every leg. The chosen journeys then lie on one cheapest chain, and the alternatives of each leg are ordered by how good a chain they allow.

### Nearby days

After the plan, the planner solves each chain again with every date moved a day earlier and a day later. When a leg's best journey on the other day costs at least 90 minutes and 15% less on its own, the planner mentions that day. This is a search over one more discrete choice, the date, which the traveller makes by hand because moving a leg changes the nights around it.

## Heuristics and limitations

The planner is exact for part of the cost and a heuristic for the rest. This section lists where it departs from the stated problem.

### Pruning before costing

The Pareto search drops a journey when another leaves no earlier, arrives no later and uses no more trains. That is only safe when a lower cost always comes with a later departure, an earlier arrival or fewer trains. Three terms break that condition:

1. **Arriving too early.** The search always prefers the earlier arrival, while the cost can prefer a later one that falls inside the arrival window.
2. **Sleep.** Staying on a night train to a later stop arrives later, yet sleeps longer and can cost less. For example, a traveller with a wake-up time of 09:30 on a night train that reaches Basel at 06:21 and Zürich at 08:05 sleeps longer by staying on to Zürich, but the plain search keeps only the journey that gets off in Basel, because it arrives earlier with the same number of trains.
3. **Ways to and from stations.** The search counts them as travel time, while the cost weighs them one and a half times, so a journey from a closer station can be dropped in favour of one that is a few minutes faster from a farther station.

The second night search is a patch for the second case: its extra constraints remove the seated journeys that would push a night train out of the set, and the rule about leaving a berth keeps the journeys that stay on board. The 3-hour limit on boarding seated trains is a guess about when the night ends for a traveller who slept on a train, and it can still let a seated overnight journey through, or keep a traveller from an early seated train after a night train.

An exact method gives the search the cost as a fourth criterion, as McRAPTOR adds criteria to RAPTOR (Delling et al., 2012). Each station then keeps the labels that no other label beats on arrival time, cost so far and number of trains, and pruning stays safe for every term that the search can add up along the path. For the arrival window, the term is added when a label reaches the arrival place. McRAPTOR is usually a few times slower than RAPTOR. With single searches taking between 5 and 50 milliseconds, that is affordable.

### The sleep term

The sleep term takes the single train with berths that covers most of the night, because changing trains during the night wakes the traveller up. A sum over rides would fit the standard form and a label-based search, and differs only for journeys that change between two trains with berths during the night, which are rare and poor anyway.

### Chains

Each later leg of a chain is searched only from the earliest arrival plus the shortest stay. A stay a little shorter than the minimum is never searched, even when it would fit the next place's arrival window much better, so the penalty for a short stay never gets to weigh it. The dynamic programming across the chain is exact for the journeys the searches found, but the searches themselves are the limit.

### Walking and stations

The ways between a place and a station, and between nearby stations, are estimates from the straight-line distance: walking at 5 km/h with a detour factor of 1.3 below one kilometre, and above that 15 minutes plus the distance at 15 km/h for local transport. The planner doesn't route on streets or local networks. The search considers the 16 closest stations within the place's search radius, and walks between stations take one hop, so a walk from one station through a second to a third only exists when the first and the third are close enough on their own.

### The timetable

The costs are only as good as the timetable. Stations from different feeds are matched by code, by name and distance, and by distance alone, and copies of the same train in different feeds are merged. A mistake in either step shows up as a train that is missing, a change that isn't possible in reality, or a change that the traveller doesn't need. A station that the Trainline dataset lists as part of another only joins it when the two lie within one kilometre, because the dataset also files separate stations under a main station, such as an airport station under the city's main station.

The minimum time for a change is the traveller's setting at every station. Feeds that give a longer time for particular stations or trains are not followed, so a change that the setting allows can be too short in reality. This matters most for trains with check-in, such as Eurostar, which closes boarding well before departure.

### Comparison with MOTIS

MOTIS, run on the same rail feeds with its own benchmark queries, gives a check on both the search and the timetable. Of 500 random queries between stops, 363 have the same earliest arrival. In 84 more, the planner arrives earlier by walking or taking local transport between stations up to five kilometres apart, which MOTIS without street data doesn't link. Of the remaining differences, most are a few minutes and come from the minimum change time above, and the rest from trains that only one of the two finds in the feeds.

## References

- D. Delling, T. Pajor, R. F. Werneck. Round-Based Public Transit Routing. ALENEX 2012, and Transportation Science 49(3), 2015. Describes RAPTOR, rRAPTOR and McRAPTOR.
- J. Dibbelt, T. Pajor, B. Strasser, D. Wagner. Intriguingly Simple and Fast Transit Routing. SEA 2013. Describes the Connection Scan Algorithm.
- S. Witt. Trip-Based Public Transit Routing. ESA 2015.
- S. Irnich, G. Desaulniers. Shortest Path Problems with Resource Constraints. In Column Generation, Springer, 2005.
- MOTIS and its routing core nigiri, at [github.com/motis-project](https://github.com/motis-project).
