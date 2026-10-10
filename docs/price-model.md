# The ticket price model in BTT

This document describes how BTT estimates the price of a train ticket when no published tariff or live price covers it. It first says what the model estimates, then states the model and how it learns from live prices, and it ends with what it leaves out.

## The estimate

The optimizer prices every run of consecutive rides within a day, because one ticket can cover several trains. A run goes through a list of price sources, and the first that knows the run gives its price. Inside Luxembourg travel is free, and a published tariff, such as the ČD basic fare for domestic Czech trains, prices the runs of its country. The price model is the last source and prices every run.

The model estimates the lowest single fare for one adult in second class. That is what the booking sites return: the lowest price DB, ÖBB and SBB show for a connection, and the lowest offer of Trenitalia, Renfe and the others. The estimate is a range that should hold 80 percent of such fares. The optimizer decides with the middle of the range, like it does with every other range.

The app looks up live prices for the tickets it shows, and each one becomes an observation. Over time the model therefore learns the prices of the trains the traveller actually looks at.

## The model

### A run as the model sees it

The model reads four things from a run of rides.

1. **Operator.** The operator of the longest ride, such as `db-fernverkehr` or `regiojet`. `data/operators.toml` gives every ride its operator from the agency of its trip.
2. **Kind of train.** One of high speed, long distance, regional, night and bus. A run takes the kind of its fastest train, and a train with couchettes or sleepers makes it a night run.
3. **Country.** For a run inside one country, that country, whoever runs the trains. A German ICE between Basel and Olten costs what Swiss trains cost. For a run across a border, the home country of the operator, which usually sells the ticket.
4. **Distance.** The total length of the rides along their lines. A run across a border is also marked as such, because international tickets often carry a surcharge.

The kind comes from `data/fares/price_model.toml`, which lists the categories of each kind, such as `ICE`, `TGV INOUI` and `FR` for high speed and `IC`, `EC` and `RJX` for long distance. Categories compare without regard to case, and a rule can be limited to some operators. When no rule matches, an operator can have a kind of its own, such as high speed for every OUIGO train. Anything else is regional, which is the right answer for the hundreds of local categories in the timetable feeds.

### Log price and distance

The model works on the logarithm of the price. Fares grow by percentages, not by euros, a log price can never turn into a negative price, and the spread of fares around their typical value is about the same share at every distance. That last point matters, because the regression below assumes the noise has one size.

The distance enters as

$$
u = \log \frac{\text{km} + k_0}{\text{km}_\text{ref} + k_0},
$$

with a reference distance $\text{km}_\text{ref}$ of 100 km. The shift $k_0$ stands in for a base fee. A tariff of the form $\text{base} + \text{rate} \cdot \text{km}$ equals $\text{rate} \cdot (\text{km} + \text{base}/\text{rate})$, so with $k_0 = \text{base}/\text{rate}$ a log price of $a + u$ reproduces it exactly. The ČD fare of 17 crowns plus 2.04 per kilometre gives $k_0 = 8.3$. A slope on $u$ below 1 makes the price grow more slowly over long distances, as national tariffs do. Each country has its own $k_0$, 10 km by default, and the model does not learn it, because it is the one parameter that does not enter linearly. Dividing by the reference distance makes every intercept the log price of a 100 km run, so intercepts and slopes are close to independent in the prior.

### Terms

The log price of a run is a sum of terms:

$$
\begin{aligned}
\log p = {} & (a + b\,u) && \text{global} \\
& + (a_c + b_c\,u) && \text{country } c \\
& + (o_\text{op} + e_\text{op}\,u + f_\text{op}\,v + q_\text{op}\,u^2) && \text{operator} \\
& + (a_k + b_k\,u) && \text{kind } k \\
& + d_{c,k} && \text{kind } k \text{ in country } c \\
& + g_{\text{op},\text{cat}} && \text{category of the operator} \\
& + s \cdot \text{border} + \varepsilon.
\end{aligned}
$$

Each term is a one-hot feature times a weight, so this is one linear regression $\log p = x^\top w + \varepsilon$ with a sparse feature vector of about fifteen nonzero entries.

The split follows how prices differ in Europe. The level of prices differs a lot between countries: a Polish train costs a fraction of a Swiss one over the same distance. How price changes with the kind of train and the border is more alike. The country terms therefore carry the level, and the kind terms carry the shape, which every country shares. A country where the app has only seen regional prices still gets a sensible high-speed price, from its own level and the ratio between the kinds that other countries show. The country and kind term lets one country depart from the shared ratio when its data insist, and the operator terms do the same for an operator that prices unlike its country, such as OUIGO next to SNCF. An operator has the shape of its own tariff too, because operators that set their own fares differ in more than the level. The slope $e_\text{op}$ is its price per kilometre. The term $f_\text{op}\,v$, with $v = 10 / (\text{km} + 5)$, is its base fare: the log price of $\text{base} + \text{rate} \cdot \text{km}$ lies above that of $\text{rate} \cdot \text{km}$ by about $\text{base} / (\text{rate} \cdot \text{km})$, and $v$ follows that shape, from 1 at 5 km to 0.1 at 95 km. The curve $q_\text{op}\,u^2$ lets its tariff taper on long trips. On the Dutch prices, where each regional operator has its own base fare and price per kilometre, these terms bring the median error from a factor of 1.083 to 1.043. The category term separates trains of one operator, such as `ICE` and `ICE Sprinter`, and is keyed by operator because a category like `IC` means a different train in different countries.

### Priors

Every weight has a normal prior, independent of the others:

| Weight | Prior mean | Prior standard deviation |
|---|---|---|
| Global intercept $a$ | log price of 100 km at 0.11 euro per km | 0.2 |
| Global slope $b$ | 1 | 0.05 |
| Country intercept $a_c$ | the country's level from the file, else 0 | 0.35, or 0.1 for a published tariff |
| Country slope $b_c$ | 0 | 0.1 |
| Operator $o_\text{op}$ | from the file, such as −0.4 for OUIGO, else 0 | 0.15 |
| Operator slope $e_\text{op}$ | 0 | 0.1 |
| Operator base fare $f_\text{op}$ | 0 | 0.3 |
| Operator curve $q_\text{op}$ | 0 | 0.1 |
| Kind intercept $a_k$ | from 0.3 for night to −0.3 for bus | 0.2 |
| Kind slope $b_k$ | 0 | 0.05 |
| Kind in a country $d_{c,k}$ | 0 | 0.1 |
| Category $g_{\text{op},\text{cat}}$ | 0 | 0.1 |
| Border $s$ | 0 | 0.2 |

A standard deviation of 0.2 on a log price lets a term move the price by about 20 percent before the data have to argue for more. The country levels come from the published tariff for Czechia, from the fare structure for the Netherlands and Belgium, and from rough figures elsewhere. The priors also settle what the data alone cannot. A high country level with a low kind offset fits the same prices as a low level with a high offset, and the tighter prior on the kind terms puts the difference in the country.

The priors set the width of the range before any observation, because a prediction adds up the variances of its terms. For a country with no observations the range spans a factor of about 4 from low to high, which is honest about how little the model knows.

### Bayesian linear regression

With a normal prior $w \sim \mathcal{N}(m_0, \Lambda_0^{-1})$, where $\Lambda_0$ is diagonal, and normal noise $\varepsilon \sim \mathcal{N}(0, \sigma^2)$, the posterior of the weights is normal too. The model keeps it in two forms. The precision matrix $\Lambda$ and the information vector $h = \Lambda \mu$ are the exact state. An observation $(x, y)$ adds to them:

$$
\Lambda \leftarrow \Lambda + \frac{x x^\top}{\sigma^2}, \qquad h \leftarrow h + \frac{x\,y}{\sigma^2}.
$$

The mean $\mu = \Lambda^{-1} h$ and the covariance $\Sigma = \Lambda^{-1}$ are kept beside them, so a prediction needs no solve. After an observation they follow from the Sherman-Morrison formula, which costs $O(d^2)$ for $d$ weights:

$$
v = \Sigma x, \qquad
\mu \leftarrow \mu + \frac{y - x^\top \mu}{\sigma^2 + x^\top v}\, v, \qquad
\Sigma \leftarrow \Sigma - \frac{v v^\top}{\sigma^2 + x^\top v}.
$$

Without forgetting, this is recursive least squares with a prior. A prediction for a run with features $x$ has mean $x^\top \mu$ and variance $x^\top \Sigma x + \sigma^2$, the uncertainty of the weights plus the spread of single fares. Both sums only run over the nonzero entries of $x$. The range is the mean plus and minus $z \cdot$ the standard deviation, with $z = 1.28$ for 80 percent, taken back from the log. The median of the fare is $\exp(x^\top \mu)$, and the middle of the range lies above it because the range is wider upwards in euros.

A weight only enters the matrices when an observation first involves it. Until then it is at its prior and uncorrelated with the rest, so a prediction adds its prior mean and variance directly. A new row and column of $\Lambda$ and $\Sigma$ hold only its prior, which is exact, because no observation has linked it to anything. The matrices therefore grow with the operators, countries and categories the app has seen, not with every category in the timetable. The linear algebra uses `nalgebra`.

## Updates from live prices

### Observations

Each live price of a ticket for one adult in second class becomes an observation. The ticket's request already carries the run, with the operator, category, countries, distance and berths of each ride, so the observation needs nothing the optimizer did not know. Tickets for other travellers carry no run, because their fares follow rules the model does not know, such as youth fares.

Observations go into the table `price_observations` of the app database, with the stations, the local departure, the day of the lookup, the price in euro cents, the price as the site gave it, the name of the site and the rides as JSON. The table holds the facts of the lookup and nothing derived from them, so a change of the kind table or the terms reads old rows anew. One route keeps one price per departure and day of lookup, so a ticket looked up again, and a popular route looked up many times, counts once per day. Rows older than two years are deleted when the app starts, because forgetting has made them count for almost nothing.

### Decay of old observations

Prices change with the yearly tariff and with demand, so older observations should count less. Before an observation from a later day, the posterior moves towards the prior:

$$
\Lambda \leftarrow \lambda \Lambda + (1 - \lambda) \Lambda_0, \qquad
h \leftarrow \lambda h + (1 - \lambda) \Lambda_0 m_0, \qquad
\lambda = 2^{-\Delta / 182},
$$

where $\Delta$ is the number of days since the last observation, so data lose half their weight in half a year. Two choices in this rule matter. The decay follows days, not observations, because otherwise a busy operator would age the data of a rare one. And it moves towards the prior instead of only scaling the precision down, which keeps weights that nobody observes at their prior variance instead of letting it grow without bound. The decay adds a term of full rank, so the mean and covariance are then computed again from $\Lambda$ with a Cholesky factorization. That also clears the rounding errors the rank-one updates collect. It happens once per day with observations, not once per observation.

### Noise and outliers

The noise $\sigma^2$ is one number per kind of train in each country, because countries differ a lot in it. Dutch fares lie within a few percent of the tariff, and Swiss ones scatter by a third through zone fares and saver fares. The noise starts at a standard deviation of 0.3 on the log price, which counts as 20 observations, and becomes the mean of that prior and the squared residuals. The residual it learns from is the one after the update, $y - x^\top \mu$ with the new mean. A residual before the update would still carry the error of a prior that the data have since corrected, and keep the noise high long after. The residual after the update is smaller than the noise by the share of the observation the weights took up, its leverage $x^\top \Sigma x / \sigma^2$, so it is divided by one minus the leverage. The noise decays with the same factor as the weights.

A booking site sometimes returns a price that is no fare, such as a promotion of one euro or a price for part of the trip. An observation more than three standard deviations from the prediction counts as if its noise were large enough to put it at three, so it moves the model by a bounded amount. Prices below 50 cents are left out, because their logarithm says nothing about a fare.

### Start of the app

The posterior is not stored. At start, the app builds it from three sources in order.

1. The priors in `data/fares/price_model.toml`.
2. The seed observations in `data/fares/price_seed.jsonl`, compiled into the app.
3. The rows of `price_observations`, in the order of their lookup.

Each source goes through the same update as a live price. A change of the priors, the kind table or the terms needs no migration, since the next start learns everything again under the new layout. The model then decays to the current day.

## The seed

The seed gives the model observations for the countries the app has live prices for before the traveller has looked anything up. The seed run is a mode of the app. With `BTT_PRICE_SEED_OUT` set to a file, the app picks rides across the timetable bundle, prices each through the same live lookups as the Tickets screen, writes every price as a line of JSON and exits. `BTT_PRICE_SEED_RIDES` sets the number of rides per operator, `BTT_PRICE_SEED_OPERATORS` the operators, `BTT_PRICE_SEED_DAYS` the days ahead of the departures and `BTT_PRICE_SEED_RANDOM` the seed of the random choice.

The rides are single trains of the operators that a booking site with live prices sells, each from a random stop to a later stop of a random trip, departing 3, 10, 21 or 40 days after the run unless set otherwise. The random generator has a fixed seed by default, so the same bundle gives the same rides, and another seed gives other rides. The operators take turns, because a site that fails once refuses requests for fifteen minutes. A ride whose site is paused waits for it and is tried up to three times, and meanwhile the other operators go on. The run leaves the app database alone, so the seed and the traveller's own prices stay apart. It runs by hand, outside the build of the timetable, because live prices change between runs and the build has to be reproducible. Running it again after a change of tariffs is enough.

The seed in the repository holds 1179 prices of 31 operators, looked up on 8 October 2026. It comes from runs over all operators and from runs for the Netherlands, Switzerland, Poland and the operators with mostly fixed fares. Austria and Italo have almost nothing, because their booking sites failed during the runs.

`cargo run -p btt-fares --example price_seed_report` checks a seed file. It holds out every fifth observation, learns from the rest and reports the median error of the prediction and the share of held-out prices inside the range, per country and per kind, next to the same numbers for the model at its prior.

### Accuracy

`cargo run -p btt-fares --example price_experiments -- <prices.jsonl>` compares sets of terms by cross-validation in five folds, with every route in one fold only. The error is the factor between the estimate and the live price, and the median error is the factor that half the prices stay within. The model learns online in each fold, as in the app.

| Prices | Count | Median error | Prices inside the range |
|---|---|---|---|
| The Netherlands, NS and six regional operators | 411 | 1.043 | 94 percent |
| Poland, PKP Intercity and four regional operators through Koleo | 187 | 1.111 | 86 percent |
| Switzerland, SBB, BLS, SOB and RhB | 231 | 1.200 | 84 percent |
| Italy, Trenitalia | 72 | 1.199 | 82 percent |
| Denmark, DSB | 36 | 1.442 | 72 percent |
| All operators, one run | 258 | 1.291 | 79 percent |

Dutch and Polish fares follow published tariffs, and the model learns them to within a few percent. The terms for the base fare and the price per kilometre of each operator matter most there, because each regional operator sets its own. Most Dutch prices hit the tariff almost exactly and a few lie well off it, so a normal range wide enough for the few holds more than 80 percent of the rest.

Italian regional fares are set by each region, such as about one euro for a short trip in Lazio and a tourist fare of eight euro on the Cinque Terre line, which the model can only learn per operator. Danish saver fares vary a lot from train to train.

In Switzerland, no extra term moves the error by more than the noise. The days ahead, the time of day, the weekend, a curve in the distance, a shift of 0, 5 or 20 km, hinges in the distance curve at 5, 15 and 40 km, and an offset per station or per region of about 25 km all land between 1.19 and 1.24, with mean errors of the log price between 0.238 and 0.262. A fit on all prices leaves a spread of about 0.3 on the log price within every group of distance, days ahead, hour, category and operator. The error therefore comes from what the model does not see. Short Swiss trips cost the fare of the zones of a local tariff network, which does not follow the distance, and a saver fare can halve the price of a train when the site has one left.

The same experiments on all 1198 prices of the seed test the time of day. A band of the day and a weekend term for every train leave the median error at 1.146 against 1.138 without them. The same terms for each operator give 1.149, for each kind of train 1.143, and for both together 1.146. The mean error of the log price goes from 0.217 to between 0.219 and 0.223, so the terms add noise and no signal. The residuals of the regression have a mean between -0.07 and 0.06 in every band of three hours, against a spread of 0.21 to 0.33 within the band. Night trains all leave in the evening, so the time of day cannot separate them.

The days ahead get the same test with bands of 0 to 2, 3 to 7, 8 to 14, 15 to 29 and 30 or more days, because a saver fare opens and closes at a step and not along a curve. The seed holds 30 different days ahead from 1 to 77, with at least 165 prices in every band. The bands for all trains give a mean error of 0.218 against 0.217 without them. The bands for each operator give 0.214 and a median error of 1.137 against 1.138, and a smooth term for each operator gives 0.215. The bands for each country give 0.217. The best of them lowers the mean error by 1.4 percent, which is within the noise of five folds. A day train therefore gets no use from the days ahead in this data.

### Other kinds of model

`cargo run --release -p btt-fares --example price_model_families -- <prices.jsonl>` compares the regression with learners that read more inputs than it does. It uses the 1198 prices of the seed in five folds by route. The regression is fitted in one batch with the terms of the model. The other models take its residuals, which come from an inner cross-validation so that they are as large as the residuals on unseen prices.

| Model | Median error | Mean error of the log price |
|---|---|---|
| Regression of the model | 1.129 | 0.203 |
| Regression and boosted trees of depth 2 | 1.124 | 0.198 |
| Regression and boosted trees of depth 4 | 1.122 | 0.200 |
| Regression and boosted trees of depth 6 | 1.128 | 0.205 |
| Regression and a Gaussian process | 1.133 | 0.203 |
| Regression and the average of the process and the trees | 1.121 | 0.198 |
| Boosted trees alone | 1.129 | 0.218 |

The trees read the distance, the country, the operator, the kind, the category, the days ahead, the hour, the weekday, the border and the estimate of the regression. The process reads the same numbers with the operator and the country as one-hot columns. None of them beats the regression by more than 2.5 percent on the mean error, and the trees alone are only 7 percent worse. With these inputs the regression leaves about as much error as any of the learners. A better model would need an input that the seed does not record, such as how many saver fares a train has left.

The range is the low and the high price that the app shows for an estimate, and it aims to hold 80 percent of the live prices. The same example compares the shipped range with ranges from the residuals of the regression. The shipped range holds 85 percent of the prices at a mean width of 0.661 on the log scale. The 10th and the 90th percentile of the residuals, scaled by the spread of the operator, hold 80 percent at a width of 0.572. The spread of an operator is shrunk towards the spread of all prices by the number of prices that the operator has. The shipped range cut to 80 percent would be about 0.59 wide, so the gain is 3 percent.

The ranges from residuals also fail for an operator without prices. With every operator in one fold only, the test part holds operators that the training part has no price of. The ranges from residuals then hold 50 percent of the prices, because they know no spread for the operator. The shipped range holds 88 percent at a width of 1.263. The posterior scaled by the quantiles of the standardized residuals holds 81 percent at a width of 1.136, which is no narrower than the shipped range cut to the same coverage. The shipped range stays. It holds 85 percent where it aims for 80, so its multiplier is a little high, and that is the cheapest thing to tune.

## Cost

A prediction is a few hash lookups and about a hundred multiplications over the nonzero entries of the feature vector. An observation costs one rank-one update of the $d \times d$ covariance, and a day with observations one Cholesky factorization of $\Lambda$, which is $d^3/3$ multiplications. With a few hundred weights, the state is under a megabyte. In a release build, learning from 944 seed prices takes 64 ms, about 70 microseconds per observation, and a prediction takes about 3 microseconds. Learning from the seed and a year of the traveller's lookups at start therefore takes a fraction of a second. The price book shares the model with the code that feeds it, behind a read-write lock, so planning tickets and learning from a lookup do not wait on each other for long.

## Limitations

### Fares the model does not separate

The model predicts one fare per run, the lowest a site shows. Saver fares depend on how far ahead the ticket is bought, on the time of day and on the weekday, and a site shows the saver fare when it has one. The observations keep the departure and the day of the lookup, but no term uses them, so the noise of the dynamic operators holds that spread. A day-ahead price and one bought a month before fall into the same range.

NS sells fixed fares, and the model treats them as lowest fares like the rest. For the Netherlands this is right, because the fixed fare is the lowest.

### Night trains

A Nightjet or a EuroNight does not follow the distance. Its cheapest seat costs 25 to 50 euro whether the route is 500 or 1200 kilometres long, and the general model, which prices a night train by its distance like a day train, was off by a median factor of 3.7. A run in which one of these trains is the longest ride is therefore priced from the offers of the ÖBB shop, and the accommodation of a night ride is priced from them as well. `docs/night-train-fares.md` describes the fit. Night trains that the shop does not sell, such as the Intercités de nuit, still use the general model.

### Runs with several operators

A run takes the operator and category of its longest ride and the kind of its fastest. A ticket that covers a regional train of one operator and a long-distance train of another is priced as the long-distance operator's train over the whole distance. Through tickets mostly work that way, but a site that sells the two rides separately within one ticket can come out higher.

### Fixed shift

The shift $k_0$ is set by hand per country, from a tariff where one is known and 10 km elsewhere. A country whose base fee differs much from that gets short rides that are too cheap or too dear. The base fare term of each operator makes up for most of it once the operator has observations, and the country slope for some of the rest.

### Width of the range

The range comes from a normal distribution of the log price. Saver fares give fare distributions with a long tail towards cheap prices, which a symmetric range in log space does not follow. The optimizer decides with the middle of the range, which is above the median and rises with the uncertainty. A long run with an uncertain prediction can therefore look dearer than two shorter runs, and the optimizer then splits a ticket that one ticket would cover. In countries with fixed tariffs the opposite happens: most prices hit the tariff and a few lie well off it, so the range holds more than its share.

### Prior levels

The country levels in the prior are rough outside the countries with a published tariff, and the seed covers only the countries a booking site with live prices sells. Belgium, Slovakia and the Baltic states stay at their prior until a lookup or a published tariff covers them.

## References

- C. M. Bishop. Pattern Recognition and Machine Learning, chapter 3. Springer, 2006. Bayesian linear regression with a normal prior.
- S. Haykin. Adaptive Filter Theory, 4th edition. Prentice Hall, 2002. Recursive least squares with exponential forgetting.
- J. Sherman, W. J. Morrison. Adjustment of an Inverse Matrix Corresponding to a Change in One Element of a Given Matrix. Annals of Mathematical Statistics 21(1), 1950.
- R. Kulhavý, M. B. Zarrop. On a General Concept of Forgetting. International Journal of Control 58(4), 1993. Forgetting towards a prior instead of plain exponential forgetting.
- P. J. Acklam. An algorithm for computing the inverse normal cumulative distribution function, 2003. The quantile for the width of the range.
