# The ticket price model in BTT

This document explains how BTT estimates the price of a train ticket when it has no published tariff and no live price for it. We start with what exactly the model estimates. Then we build the model up one group of terms at a time, look at how it learns from the live prices the app sees, and measure how well it does on the prices we have. The last part lists what the model leaves out.

## The estimate

When the app prices a trip, the ticket optimizer looks at every run of consecutive rides within a day, because a single ticket can cover several trains. For each run it asks a list of price sources in turn, and the first source that knows the run gives the price. Travel inside Luxembourg is free, so that source comes first. Next come published tariffs, such as the ČD basic fare for domestic Czech trains, which price the runs of their own country. The price model is the last source in the list, so it has to price every run that the others leave.

What should the model estimate? We choose the lowest single fare for one adult in second class, because that is what the booking sites return. DB, ÖBB and SBB show the lowest price for a connection, and Trenitalia, Renfe and the others show their lowest offer. A single number would hide how unsure the model is, so the estimate is a range that should hold 80 percent of such fares. The optimizer decides with the middle of the range, as it does with every other range.

The app also looks up live prices for the tickets it shows, and each live price becomes an observation that the model learns from. Over time the model therefore gets best at the trains the traveller looks at.

## The model

### A run as the model sees it

A run can be long and mix several trains, but the model reduces it to four facts.

1. **Operator.** The operator of the longest ride, such as `db-fernverkehr` or `regiojet`. `data/operators.toml` gives every ride its operator from the agency of its trip.
2. **Kind of train.** One of high speed, long distance, regional, night and bus. A run takes the kind of its fastest train, and a train with couchettes or sleepers makes it a night run.
3. **Country.** For a run inside one country this is that country, whoever runs the trains. A German ICE between Basel and Olten costs what Swiss trains cost. For a run across a border it is the home country of the operator, which usually sells the ticket.
4. **Distance.** The total length of the rides along their lines. The model also notes whether the run crosses a border, because international tickets often cost extra.

The kind comes from `data/fares/price_model.toml`, which lists the categories of each kind. For example, `ICE`, `TGV INOUI` and `FR` are high speed, and `IC`, `EC` and `RJX` are long distance. Categories compare without regard to case, and a rule can be limited to some operators. When no rule matches, an operator can have a kind of its own, such as high speed for every OUIGO train. Everything else counts as regional. That sounds crude, but it is the right answer for the hundreds of local categories in the timetable feeds.

### Log price and distance

The model works with the logarithm of the price instead of the price itself, for three reasons. Fares grow by percentages rather than by euros, so a step in log price means the same for a cheap fare and a dear one. A log price can take any value and still turn back into a positive price. And the spread of fares around their typical value is about the same share at every distance. That last point matters, because the regression below assumes that the noise has one size.

How should the distance enter? The log of the distance is a natural first guess, but it would make a 1 km ride almost free. Real tariffs charge a base fee on top of a price per kilometre, so we shift the distance before we take the log:

$$
u = \log \frac{\text{km} + k_0}{\text{km}_\text{ref} + k_0},
$$

with a reference distance $\text{km}_\text{ref}$ of 100 km. To see why the shift $k_0$ stands in for a base fee, take a tariff of the form $\text{base} + \text{rate} \cdot \text{km}$. We can write it as $\text{rate} \cdot (\text{km} + \text{base}/\text{rate})$. With $k_0 = \text{base}/\text{rate}$, a log price of $a + u$ therefore reproduces the tariff exactly. The ČD fare of 17 crowns plus 2.04 per kilometre, for example, gives $k_0 = 8.3$. A slope on $u$ below 1 then makes the price grow more slowly over long distances, as national tariffs do.

Each country has its own $k_0$, and 14 km is the default. The model does not learn $k_0$, because it is the one parameter that does not enter linearly. Dividing by the reference distance makes every intercept the log price of a 100 km run. That keeps the intercepts and the slopes close to independent in the prior.

### Terms

The model builds the log price of a run as a sum of terms:

$$
\begin{aligned}
\log p = {} & (a + b\,u + f\,v) && \text{global} \\
& + (a_c + b_c\,u + f_c\,v + q_c\,u^2) && \text{country } c \\
& + (o_\text{op} + e_\text{op}\,u + f_\text{op}\,v + q_\text{op}\,u^2) && \text{operator} \\
& + (a_k + b_k\,u) && \text{kind } k \\
& + d_{c,k} && \text{kind } k \text{ in country } c \\
& + g_{\text{op},\text{cat}} + h_{c,\text{cat}} && \text{category of the operator and of the country} \\
& + (s\,v + t_\text{op}) \cdot \text{border} + \varepsilon.
\end{aligned}
$$

Each term is a one-hot feature times a weight. Together they form one linear regression $\log p = x^\top w + \varepsilon$, with a sparse feature vector of about twenty nonzero entries. The model leaves out any term whose prior spread in `data/fares/price_model.toml` is 0.

Why split the price this way? The split follows how prices differ across Europe. The level of prices differs a lot between countries, and a Polish train costs a fraction of a Swiss one over the same distance. How the price changes with the kind of train is much more alike. In most countries a high-speed train costs more than a regional one by a similar ratio. So the country terms carry the level, and the kind terms carry the shape that every country shares. Suppose the app has only seen regional prices in some country. It still gets a sensible high-speed price there, from the country's own level and the ratio between the kinds that other countries show. When the data of one country insist on a different ratio, the term $d_{c,k}$ lets that country depart from the shared one.

The operator terms do the same for an operator that prices unlike its country, such as OUIGO next to SNCF. An operator that sets its own fares usually differs in more than the level, so it also gets the shape of its own tariff. The slope $e_\text{op}$ is its price per kilometre. The term $f_\text{op}\,v$, with $v = 10 / (\text{km} + 5)$, is its base fare. To see why, compare the log price of $\text{base} + \text{rate} \cdot \text{km}$ with that of $\text{rate} \cdot \text{km}$. The first lies above the second by about $\text{base} / (\text{rate} \cdot \text{km})$, which falls off with one over the distance. The term $v$ follows that shape, from 1 at 5 km to 0.1 at 95 km. Finally, the curve $q_\text{op}\,u^2$ lets the tariff taper on long trips. These terms matter most in the Netherlands, where each regional operator has its own base fare and price per kilometre. There they bring the median error from a factor of 1.083 to 1.043.

The category term $g_{\text{op},\text{cat}}$ separates trains of one operator, such as `ICE` and `ICE Sprinter`. It is keyed by operator, because a category like `IC` means a different train in different countries.

What happens to an operator the model has no prices of? Its own terms sit at their prior, so the model has to borrow a shape from somewhere else. The base fare term $f_c\,v$ and the curve $q_c\,u^2$ of the country provide it, because they describe the tariffs of the country as a whole. The category term of the country, $h_{c,\text{cat}}$, does the same for the category. A Swiss `S` train of an operator without prices then costs what the `S` trains of SBB, BLS and SOB cost. Together these terms lower the error on operators without prices from 0.344 to 0.249 on the log price, and the curve of the country does most of that.

The border needs a term of its own. A fixed surcharge seems the obvious choice, but a short trip across a border costs about as much as a long one. The 19 km from Rzepin to Frankfurt (Oder) cost 17.50 euro, and the 23 km from Vallorbe to Frasne cost 29 euro. A fixed offset on the log price cannot fit that, because it scales the price instead of adding to it. The border surcharge is therefore a base fare $s\,v$ that fades with the distance, like the base fare of an operator. Each operator also gets an offset $t_\text{op}$, because each operator sells its international tickets by its own rules.

### Priors

Every weight has a normal prior, independent of the others:

| Weight | Prior mean | Prior standard deviation |
|---|---|---|
| Global intercept $a$ | log price of 100 km at 0.11 euro per km | 0.05 |
| Global slope $b$ | 1 | 0.025 |
| Global base fare $f$ | 0 | 0.05 |
| Country intercept $a_c$ | the country's level from the file, else 0 | 0.35, or 0.1 for a published tariff |
| Country slope $b_c$ | 0 | 0.14, or 0.1 for a published tariff |
| Country base fare $f_c$ | 0 | 0.07 |
| Country curve $q_c$ | 0 | 0.14 |
| Operator $o_\text{op}$ | from the file, such as −0.4 for OUIGO, else 0 | 0.21 |
| Operator slope $e_\text{op}$ | 0 | 0.1 |
| Operator base fare $f_\text{op}$ | 0 | 0.3 |
| Operator curve $q_\text{op}$ | 0 | 0.1 |
| Kind intercept $a_k$ | from 0.3 for night to −0.3 for bus | 0.2 |
| Kind slope $b_k$ | 0 | 0.05 |
| Kind in a country $d_{c,k}$ | 0 | 0.2 |
| Category of the operator $g_{\text{op},\text{cat}}$ | 0 | 0.14 |
| Category of the country $h_{c,\text{cat}}$ | 0 | 0.4 |
| Border base fare $s$ | 0 | 1.0 |
| Border offset of the operator $t_\text{op}$ | 0 | 0.42 |

How should we read these numbers? A standard deviation of 0.2 on a log price lets a term move the price by about 20 percent before the data have to argue for more. The spreads come from the tuning that the section on tuning of the priors describes. The global terms have tight priors, because every price has a country and the country terms carry the level. The country levels come from the published tariff for Czechia, from the fare structure for the Netherlands and Belgium, and from rough figures elsewhere.

The priors also settle what the data alone cannot. A high country level with a low kind offset fits the same prices as a low level with a high offset, so the data cannot tell the two apart. The kind terms have a tighter prior than the country terms, and that puts the difference in the country.

Before any observation, the priors also set the width of the range, because a prediction adds up the variances of its terms. For a country without observations the range spans a factor of about 4 from low to high. That is wide, and it should be, given how little the model knows there.

### Bayesian linear regression

The model has to learn from one live price at a time, and it should know how sure it is of each weight. Bayesian linear regression does both. With a normal prior $w \sim \mathcal{N}(m_0, \Lambda_0^{-1})$, where $\Lambda_0$ is diagonal, and normal noise $\varepsilon \sim \mathcal{N}(0, \sigma^2)$, the posterior of the weights is normal too. We therefore only need its mean and its covariance.

The model keeps the posterior in two forms. The precision matrix $\Lambda$ and the information vector $h = \Lambda \mu$ are the exact state, and an observation $(x, y)$ adds to them:

$$
\Lambda \leftarrow \Lambda + \frac{x x^\top}{\sigma^2}, \qquad h \leftarrow h + \frac{x\,y}{\sigma^2}.
$$

A prediction needs the mean $\mu = \Lambda^{-1} h$ and the covariance $\Sigma = \Lambda^{-1}$. Solving for them after every observation would be slow, so the model keeps them beside $\Lambda$ and $h$ and updates them with the Sherman-Morrison formula. That costs $O(d^2)$ for $d$ weights:

$$
v = \Sigma x, \qquad
\mu \leftarrow \mu + \frac{y - x^\top \mu}{\sigma^2 + x^\top v}\, v, \qquad
\Sigma \leftarrow \Sigma - \frac{v v^\top}{\sigma^2 + x^\top v}.
$$

Without forgetting, this is recursive least squares with a prior. A prediction for a run with features $x$ has mean $x^\top \mu$ and variance $x^\top \Sigma x + \sigma^2$. The first part of the variance is the uncertainty of the weights, and the second is the spread of single fares. Both sums only run over the nonzero entries of $x$. The range is the mean plus and minus $z$ times the standard deviation, with $z = 1.28$ for 80 percent, taken back from the log. The median of the fare is $\exp(x^\top \mu)$. The middle of the range lies above the median, because the range is wider upwards in euros.

The timetable holds hundreds of categories, and most of them never get a live price. A row and a column in the matrices for each would waste memory, so a weight only enters the matrices when an observation first involves it. Until then the weight is at its prior and uncorrelated with the rest, and a prediction adds its prior mean and variance directly. When the weight does enter, its new row and column hold only its prior. That is exact, because no observation has linked it to anything yet. The matrices therefore grow with the operators, countries and categories the app has seen. The linear algebra uses `nalgebra`.

## The price table of the search

The planner's search prices millions of rides, and running the full model for each would cost more than the rest of the search. At the start of a plan the app therefore builds a `PriceTable` from the posterior mean of the model. The table has one row per class of ride. A class is the operator, the category, whether the trains have berths and the country of the boarding station. A row holds the coefficients of the log price, `constant + distance·u + curve·u² + base·b`. Here `u` is the log distance and `b` the base term, both of a ride length in kilometres. Pricing a ride then takes a few multiplications and one exponential.

The rows leave out the border surcharge, because the search prices a ride by the station it boards at. Some prices in the search are therefore a little off. The ranking prices every journey the search returns with the exact cover, so the final order is still right.

Even a logarithm and an exponential add up when the search asks for about ten million prices per leg. Each row therefore also stores its price at every whole kilometre, up to twice the longest route that uses the row, and the search interpolates between two kilometres. The result differs from the formula by at most a cent for a ride of 5 km or more. A distance past the stored prices falls back to the formula.

The table is a snapshot of the model. Prices the model learns later do not change a plan that is under way, and the app builds a new table when it loads a bundle.

Each class also gets a pass row for the searches on pass days. The row says whether a pass covers the class, by the reservation rules of the ticket optimizer, and what a ride costs on top in whole cents. That is the middle of the seat reservation fee, or of the night accommodation fee for a class with berths. A seat reservation that is optional and not wanted costs nothing. The rule of the boarding station's country stands in for the whole ride, and the ranking again prices each pass option with the exact rules of the optimizer.

## Updates from live prices

### Observations

Each live price of a ticket for one adult in second class becomes an observation. The ticket's request already carries the run, with the operator, category, countries, distance and berths of each ride. The observation therefore needs nothing that the optimizer did not already know. Tickets for other travellers carry no run, because their fares follow rules the model does not know, such as youth fares.

The app stores observations in the table `price_observations` of its database. A row holds the stations, the local departure, the day of the lookup, the price in euro cents, the price as the site gave it, the name of the site and the rides as JSON. The table keeps the facts of the lookup and nothing derived from them, so a change of the kind table or of the terms simply reads the old rows anew. One route keeps one price per departure and day of lookup. A ticket looked up again, or a popular route looked up many times, then counts once per day and does not drown out the rest. The app deletes rows older than two years when it starts, because forgetting has made them count for almost nothing.

### Decay of old observations

Prices change with the yearly tariff and with demand, so an old observation should count less than a new one. Before the model learns from an observation of a later day, it moves the posterior towards the prior:

$$
\Lambda \leftarrow \lambda \Lambda + (1 - \lambda) \Lambda_0, \qquad
h \leftarrow \lambda h + (1 - \lambda) \Lambda_0 m_0, \qquad
\lambda = 2^{-\Delta / 182},
$$

where $\Delta$ is the number of days since the last observation. Data therefore lose half their weight in half a year.

The decay follows days, not observations. If it followed observations, a busy operator would age the data of a rare one. The decay also moves towards the prior instead of only scaling the precision down. Plain scaling would let the variance of a weight that nobody observes grow without bound, while moving towards the prior keeps it at its prior variance. The decay adds a term of full rank, so the model then computes the mean and the covariance again from $\Lambda$ with a Cholesky factorization. That also clears the rounding errors that the rank-one updates collect. All of this happens once per day with observations, not once per observation.

### Noise and outliers

The noise $\sigma^2$ says how far single fares scatter around the model, and it differs a lot between countries and operators. Dutch fares lie within a few percent of the tariff. Swiss fares scatter by a third, through zone fares and saver fares. One noise for all would make the Dutch ranges too wide and the Swiss ones too narrow. A separate noise for every operator has the opposite problem, because it is a poor estimate for an operator with few prices.

The model takes the middle road with three levels of noise, and each level shrinks towards the one above it. The top level is the noise of a kind of train in every country. It starts at a standard deviation of 0.3 on the log price, which counts as 20 observations, and becomes the mean of that prior and the squared residuals. The middle level is the noise of a kind of train in one country, which starts from the noise of the kind and counts it as 10 observations. The bottom level is the noise of an operator, which starts from the noise of its kind of train in its country, also with a weight of 10. A prediction uses the noise of its operator. An operator with few prices therefore gets about the noise of its country, and an operator with many prices gets its own. With these levels, 81 percent of the prices on unseen routes fall inside the range. With the noise of a kind of train in one country alone, 86 percent would, so those ranges were wider than they needed to be.

Which residual should the noise learn from? The residual before the update still carries the error of a prior that the data have since corrected, and it would keep the noise high long after the model has learned. Each level therefore learns from the residual after the update, $y - x^\top \mu$ with the new mean. That residual errs the other way, because the weights have just moved towards the observation. They took up a share of it equal to its leverage $x^\top \Sigma x / \sigma^2$, so the model divides the squared residual by one minus the leverage. The noise decays with the same factor as the weights.

A booking site sometimes returns a price that is no fare, such as a promotion of one euro or a price for part of the trip. One such price should not drag the model far. An observation more than 2.1 standard deviations from the prediction therefore counts as if its noise were large enough to put it at 2.1, and it moves the model by a bounded amount. The model ignores prices below 50 cents, because their logarithm says nothing about a fare.

### Runs that share trains

Two runs on the same day that ride some of the same trains have fares that move together, because the site prices them from the same trains and the same day. A live price for one run therefore says something about the other, beyond what the regression learns about the terms.

The model keeps the last live runs of each day in memory, with the train, the distance and the residual of every ride. When it predicts a run, it compares the run with each kept run of the same day. The shared part $c$ is the sum of $k_A k_B$ over the trains that both runs ride, divided by $K_A K_B$, where $k$ is the distance of one ride and $K$ the distance of the run. Two identical runs give $c = 1$ and runs without a common train give $c = 0$. The covariance of the two fares around the model is

$$
\operatorname{cov} = c \, \kappa \, \sqrt{\sigma_A^2 \sigma_B^2},
$$

where $\kappa$ is `common_share` in `price_model.toml`, set by hand to 0.9. The model then does the conditional update of a Gaussian. The gain is $\operatorname{cov} / (\sigma_A^2 + v_A)$, with $v_A$ the variance of the weights at the kept run. The mean of the new run moves by the gain times the residual of the kept run, and its noise variance shrinks by the gain times the covariance. Because $\kappa < 1$, the noise never reaches zero, so a second run on the same trains still gets a range.

Runs of another day or without a common train change nothing. A later price for the same trains and day replaces the earlier one. The model keeps at most `kept_runs` runs and forgets the runs whose day has passed. The price note names the trains that the estimate leans on.

### Start of the app

The app does not store the posterior. At start, it builds the posterior from three sources in order.

1. The priors in `data/fares/price_model.toml`.
2. The seed observations in `data/fares/price_seed.jsonl`, compiled into the app.
3. The rows of `price_observations`, in the order of their lookup.

Each source goes through the same update as a live price. A change of the priors, the kind table or the terms therefore needs no migration, because the next start learns everything again under the new layout. The model then decays to the current day.

## The seed

A new install has no live prices of its own. The seed gives the model observations for the countries the app has live prices for, before the traveller has looked anything up. The seed run is a mode of the app. With `BTT_PRICE_SEED_OUT` set to a file, the app picks rides across the timetable bundle, prices each through the same live lookups as the Tickets screen, writes every price as a line of JSON and exits. `BTT_PRICE_SEED_RIDES` sets the number of rides per operator, `BTT_PRICE_SEED_OPERATORS` the operators, `BTT_PRICE_SEED_DAYS` the days ahead of the departures and `BTT_PRICE_SEED_RANDOM` the seed of the random choice.

The rides are single trains of the operators that a booking site with live prices sells. Each ride goes from a random stop to a later stop of a random trip and departs 3, 10, 21 or 40 days after the run, unless set otherwise. The random generator has a fixed seed by default, so the same bundle gives the same rides and another seed gives other rides. The operators take turns, because a site that fails once refuses requests for fifteen minutes. A ride whose site is paused waits for it and is tried up to three times, and meanwhile the other operators go on. The run leaves the app database alone, so the seed and the traveller's own prices stay apart. It runs by hand, outside the build of the timetable, because live prices change between runs and the build has to be reproducible. After a change of tariffs it is enough to run it again.

The seed in the repository holds 1198 prices of 32 operators, nearly all looked up on 8 October 2026. It comes from runs over all operators and from runs for the Netherlands, Switzerland, Poland and the operators with mostly fixed fares. Austria and Italo have almost nothing, because their booking sites failed during the runs.

`cargo run -p btt-fares --example price_seed_report` checks a seed file. It holds out every fifth observation, learns from the rest and reports the median error of the prediction and the share of held-out prices inside the range. It reports these per country and per kind, next to the same numbers for the model at its prior.

### Tuning of the priors

The priors above have about twenty spreads, and setting them by hand is guesswork. We tune them on the seed instead, with `cargo run --release -p btt-fares --example price_tuning -- <prices.jsonl> [mode]`. The example scores the model as the app runs it. It splits the prices into five folds, lets the model learn online from four of them and predicts the fifth. It repeats this over three different splits, which makes the scores less noisy.

How the prices are split into folds decides what the score measures. With folds by route, a route is in one fold only, so the score says how well the model prices a route it has not seen. That is the common case in the app, but not the only one. With folds by operator, the test part holds operators that the training part has no price of. With folds by country, it holds countries without prices. These two say how well the model borrows from the rest when it has nothing of its own.

The example has a few modes. The mode `variants` compares the shipped priors with versions that leave out one group of terms. The mode `tune` changes one spread at a time and keeps a change when it lowers the objective. The objective is the mean error of the folds by route plus half the mean error of the folds by operator. The mode `tune-density` then tunes the noise on the mean negative log density, a score that rewards a range that is narrow and still holds the price.

Why does the objective include the folds by operator? A tuning on the folds by route alone lets the spreads grow wide and drops the kind terms. That lowers the error on unseen routes to 0.181, but it raises the error on unseen operators from 0.344 to 0.399, with ranges that hold all their prices. The model then fits the operators it knows at the cost of the ones it does not. For the same reason the kind terms stay in, although the tuning drops them. Without them, a high-speed train in a country without prices is off by 0.553 instead of 0.470.

The table below compares the shipped priors with a reference model on the 1198 prices of the seed. The reference has hand-set spreads. It leaves out the global and the country base fare, the curve and the category of the country, and the border base fare and offset of the operator. It has a fixed border surcharge instead, and the noise of a kind of train in one country only. The error is the mean absolute error of the log price, and the range is the share of prices inside it.

| Folds | Median error, reference | Median error, shipped | Error, reference | Error, shipped | Range, reference | Range, shipped |
|---|---|---|---|---|---|---|
| By route | 1.113 | 1.101 | 0.201 | 0.184 | 85 percent | 81 percent |
| By route, three other splits | 1.117 | 1.106 | 0.203 | 0.186 | 85 percent | 81 percent |
| By operator | 1.294 | 1.159 | 0.344 | 0.249 | 88 percent | 95 percent |
| By country | 1.423 | 1.265 | 0.417 | 0.326 | 91 percent | 95 percent |

On unseen routes the error drops by 8 percent, and the range comes down to its target of 80 percent. On unseen operators and countries the error drops by about a quarter. The three other splits never took part in the tuning, and they show the same gain, so the tuning has not just fitted the splits it saw.

Which terms earn their place? The next table leaves out one group of terms from the shipped priors at a time and shows the error on unseen routes and on unseen operators.

| Without | Routes | Operators |
|---|---|---|
| Nothing | 0.184 | 0.249 |
| The base fare of the country and the global one | 0.184 | 0.256 |
| The curve of the country | 0.187 | 0.348 |
| The category of the country | 0.190 | 0.269 |
| The border base fare and operator, with a fixed surcharge instead | 0.187 | 0.251 |
| The noise of the kind and of the operator | 0.185, with 86 percent in the range | 0.252 |

The curve of the country matters most for unseen operators, and the category of the country matters most for unseen routes. The base fares barely move either score, but they cost little.

The weak spot is the range for an unseen operator, which holds 95 percent of the prices where it aims for 80. The prior spreads of the operator terms set that width. Narrower spreads would narrow the range, but they would also pull the estimates for operators with prices towards their country. The tuning prefers the better estimate.

### Accuracy

The experiments in this section and the next use the reference model of the previous section. The numbers are those of the reference, but the conclusions about which inputs help hold for the shipped model too.

`cargo run -p btt-fares --example price_experiments -- <prices.jsonl>` compares sets of terms by cross-validation in five folds, with every route in one fold only. The error is the factor between the estimate and the live price. The median error is the factor that half the prices stay within, so a median error of 1.1 means that half the estimates lie within 10 percent of the price. The model learns online in each fold, as in the app.

| Prices | Count | Median error | Prices inside the range |
|---|---|---|---|
| The Netherlands, NS and six regional operators | 411 | 1.043 | 94 percent |
| Poland, PKP Intercity and four regional operators through Koleo | 187 | 1.111 | 86 percent |
| Switzerland, SBB, BLS, SOB and RhB | 231 | 1.200 | 84 percent |
| Italy, Trenitalia | 72 | 1.199 | 82 percent |
| Denmark, DSB | 36 | 1.442 | 72 percent |
| All operators, one run | 258 | 1.291 | 79 percent |

Dutch and Polish fares follow published tariffs, and the model learns them to within a few percent. The terms for the base fare and the price per kilometre of each operator matter most there, because each regional operator sets its own. Why does the Dutch range hold 94 percent instead of 80? Most Dutch prices hit the tariff almost exactly, and a few lie well off it. A normal range that is wide enough for the few then holds more than 80 percent of the rest.

Italian regional fares are set by each region. A short trip in Lazio costs about one euro, and the Cinque Terre line has a tourist fare of eight euro. The model can only learn such fares per operator. Danish saver fares vary a lot from train to train.

Swiss fares are harder to explain. We tried the days ahead, the time of day, the weekend, a curve in the distance, a shift of 0, 5 or 20 km, hinges in the distance curve at 5, 15 and 40 km, and an offset per station or per region of about 25 km. None of them moves the error by more than the noise. All land between 1.19 and 1.24, with mean errors of the log price between 0.238 and 0.262. A fit on all prices leaves a spread of about 0.3 on the log price within every group of distance, days ahead, hour, category and operator. The error therefore comes from what the model does not see. Short Swiss trips cost the fare of the zones of a local tariff network, which does not follow the distance. And a saver fare can halve the price of a train when the site has one left.

Saver fares suggest that the time of day should matter, so we tested it on all 1198 prices of the seed. A band of the day and a weekend term for every train leave the median error at 1.146, against 1.138 without them. The same terms for each operator give 1.149, for each kind of train 1.143, and for both together 1.146. The mean error of the log price goes from 0.217 to between 0.219 and 0.223, so the terms add noise and no signal. The residuals of the regression have a mean between -0.07 and 0.06 in every band of three hours, against a spread of 0.21 to 0.33 within the band. Night trains all leave in the evening, so the time of day cannot separate them.

The days ahead are the other obvious candidate. A saver fare opens and closes at a step and not along a curve, so we test them in bands of 0 to 2, 3 to 7, 8 to 14, 15 to 29 and 30 or more days. The seed holds 30 different days ahead from 1 to 77, with at least 165 prices in every band. The bands for all trains give a mean error of 0.218, against 0.217 without them. The bands for each operator give 0.214 and a median error of 1.137 against 1.138, and a smooth term for each operator gives 0.215. The bands for each country give 0.217. The best of them lowers the mean error by 1.4 percent, which is within the noise of five folds. In this data, a day train therefore gets no use from the days ahead.

### Other kinds of model

Perhaps the regression is too stiff, and a more flexible learner would find structure that it misses. `cargo run --release -p btt-fares --example price_model_families -- <prices.jsonl>` tests this. It compares the regression with learners that read more inputs than it does, on the 1198 prices of the seed in five folds by route. The regression is fitted in one batch with the terms of the model. The other models learn from its residuals. Those residuals come from an inner cross-validation, so that they are as large as the residuals on unseen prices.

| Model | Median error | Mean error of the log price |
|---|---|---|
| Regression of the model | 1.129 | 0.203 |
| Regression and boosted trees of depth 2 | 1.124 | 0.198 |
| Regression and boosted trees of depth 4 | 1.122 | 0.200 |
| Regression and boosted trees of depth 6 | 1.128 | 0.205 |
| Regression and a Gaussian process | 1.133 | 0.203 |
| Regression and the average of the process and the trees | 1.121 | 0.198 |
| Boosted trees alone | 1.129 | 0.218 |

The trees read the distance, the country, the operator, the kind, the category, the days ahead, the hour, the weekday, the border and the estimate of the regression. The process reads the same numbers, with the operator and the country as one-hot columns. None of them beats the regression by more than 2.5 percent on the mean error, and the trees alone are only 7 percent worse. With these inputs the regression leaves about as much error as any of the learners. A better model would need an input that the seed does not record, such as how many saver fares a train has left.

The same example also tests the range. The range is the low and the high price that the app shows for an estimate, and it aims to hold 80 percent of the live prices. Could ranges from the residuals of the regression do better than the posterior? The shipped range holds 85 percent of the prices at a mean width of 0.661 on the log scale. The 10th and the 90th percentile of the residuals, scaled by the spread of the operator, hold 80 percent at a width of 0.572. Here the spread of an operator shrinks towards the spread of all prices by the number of prices that the operator has. Cut to 80 percent, the shipped range would be about 0.59 wide, so the gain is only 3 percent.

The ranges from residuals also fail for an operator without prices. With every operator in one fold only, the test part holds operators that the training part has no price of. The ranges from residuals then hold 50 percent of the prices, because they know no spread for the operator. The shipped range holds 88 percent at a width of 1.263. The posterior, scaled by the quantiles of the standardized residuals, holds 81 percent at a width of 1.136, which is no narrower than the shipped range cut to the same coverage. The shipped range therefore stays. With the noise of the kind and of the operator, it now holds 81 percent of the prices on unseen routes.

## Cost

A prediction is a few hash lookups and about a hundred multiplications over the nonzero entries of the feature vector. An observation costs one rank-one update of the $d \times d$ covariance, and a day with observations costs one Cholesky factorization of $\Lambda$, which is $d^3/3$ multiplications. With a few hundred weights, the state takes less than a megabyte. In a release build, learning from 944 seed prices takes 64 ms, about 70 microseconds per observation, and a prediction takes about 3 microseconds. Learning from the seed and a year of the traveller's lookups at start therefore takes a fraction of a second. The price book shares the model with the code that feeds it, behind a read-write lock, so planning tickets and learning from a lookup do not wait on each other for long.

## Limitations

### Fares the model does not separate

The model predicts one fare per run, the lowest a site shows. Saver fares depend on how far ahead the ticket is bought, on the time of day and on the weekday, and a site shows the saver fare when it has one. The observations keep the departure and the day of the lookup, but no term uses them, so the noise of the dynamic operators absorbs that spread. A price for the next day and one bought a month before fall into the same range.

NS sells fixed fares, and the model treats them as lowest fares like the rest. For the Netherlands this is right, because the fixed fare is the lowest.

### Night trains

A Nightjet or a EuroNight does not follow the distance. Its cheapest seat costs 25 to 50 euro whether the route is 500 or 1200 kilometres long. The general model prices a night train by its distance like a day train, and it was off by a median factor of 3.7. A run in which one of these trains is the longest ride is therefore priced from the offers of the ÖBB shop, and so is the accommodation of a night ride. `docs/night-train-fares.md` describes that fit. Night trains that the shop does not sell, such as the Intercités de nuit, still use the general model.

### Runs with several operators

A run takes the operator and category of its longest ride and the kind of its fastest. A ticket that covers a regional train of one operator and a long-distance train of another is therefore priced as the long-distance operator's train over the whole distance. Through tickets mostly work that way. A site that sells the two rides separately within one ticket can come out higher.

### Fixed shift

The shift $k_0$ is set by hand per country, from a tariff where one is known and 14 km elsewhere. A country whose base fee differs much from that gets short rides that are too cheap or too dear. The base fare terms of the country and of each operator make up for most of it once they have observations, and the country slope makes up for some of the rest.

### Width of the range

The range comes from a normal distribution of the log price. Saver fares give fare distributions with a long tail towards cheap prices, which a symmetric range in log space does not follow. The optimizer decides with the middle of the range, which lies above the median and rises with the uncertainty. A long run with an uncertain prediction can therefore look dearer than two shorter runs, and the optimizer then splits a ticket that one ticket would cover. In countries with fixed tariffs the opposite happens. Most prices hit the tariff and a few lie well off it, so the range holds more than its share.

### Shared trains

The value of `common_share` is a guess and no fit. The model also treats the residual of a kept run as exact, and it learns nothing from runs of past days, since the fares of another day are not the same fares.

### Prior levels

Outside the countries with a published tariff, the country levels in the prior are rough. The seed covers only the countries that a booking site with live prices sells. Belgium, Slovakia and the Baltic states therefore stay at their prior until a lookup or a published tariff covers them.

## References

- C. M. Bishop. Pattern Recognition and Machine Learning, chapter 3. Springer, 2006. Bayesian linear regression with a normal prior.
- S. Haykin. Adaptive Filter Theory, 4th edition. Prentice Hall, 2002. Recursive least squares with exponential forgetting.
- J. Sherman, W. J. Morrison. Adjustment of an Inverse Matrix Corresponding to a Change in One Element of a Given Matrix. Annals of Mathematical Statistics 21(1), 1950.
- R. Kulhavý, M. B. Zarrop. On a General Concept of Forgetting. International Journal of Control 58(4), 1993. Forgetting towards a prior instead of plain exponential forgetting.
- P. J. Acklam. An algorithm for computing the inverse normal cumulative distribution function, 2003. The quantile for the width of the range.
