<script lang="ts">
  // Edits one traveller profile.
  import type { DiscountCard, TravelClass, Traveller } from "../bindings";
  import { ageGroupName, ageOn, DISCOUNT_CARDS, RESIDENCE_COUNTRIES } from "../traveller";

  let {
    traveller,
    onChange,
    onDelete,
    today,
  }: {
    traveller: Traveller;
    onChange: (traveller: Traveller) => void;
    onDelete: () => void;
    /** The day the age group is shown for, as `YYYY-MM-DD`. */
    today: string;
  } = $props();

  const age = $derived(ageOn(traveller.birth_date, today));

  function update(changes: Partial<Traveller>) {
    onChange({ ...traveller, ...changes });
  }

  function toggleCard(card: DiscountCard, owned: boolean) {
    const others = traveller.discount_cards.filter((existing) => existing !== card);
    update({ discount_cards: owned ? [...others, card] : others });
  }
</script>

<section class="traveller">
  <div class="row">
    <label>
      <span class="label">Name</span>
      <input
        type="text"
        value={traveller.name}
        onchange={(event) => update({ name: event.currentTarget.value.trim() || traveller.name })}
      />
    </label>
    <button type="button" onclick={onDelete}>Remove</button>
  </div>
  <label>
    <span class="label">Date of birth</span>
    <input
      type="date"
      value={traveller.birth_date}
      onchange={(event) => {
        if (event.currentTarget.value !== "") {
          update({ birth_date: event.currentTarget.value });
        }
      }}
    />
    <span class="hint">{age} years, {ageGroupName(age)}</span>
  </label>
  <label>
    <span class="label">Lives in</span>
    <select
      value={traveller.residence_country}
      onchange={(event) => update({ residence_country: event.currentTarget.value })}
    >
      {#each RESIDENCE_COUNTRIES as country (country.code)}
        <option value={country.code}>{country.name}</option>
      {/each}
    </select>
  </label>
  <label>
    <span class="label">Class</span>
    <select
      value={traveller.travel_class}
      onchange={(event) => update({ travel_class: event.currentTarget.value as TravelClass })}
    >
      <option value="Second">Second class</option>
      <option value="First">First class</option>
    </select>
  </label>
  <fieldset>
    <legend class="label">Discount cards</legend>
    {#each DISCOUNT_CARDS as { card, name } (card)}
      <label class="card">
        <input
          type="checkbox"
          checked={traveller.discount_cards.includes(card)}
          onchange={(event) => toggleCard(card, event.currentTarget.checked)}
        />
        {name}
      </label>
    {/each}
  </fieldset>
</section>

<style>
  .traveller {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    padding: var(--space-3);
  }

  .row,
  label {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .row label {
    flex: 1;
  }

  .row input {
    flex: 1;
  }

  .label {
    min-width: 7em;
    color: var(--color-text-muted);
  }

  .hint {
    color: var(--color-text-muted);
  }

  fieldset {
    border: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-4);
  }

  legend {
    padding: 0;
    margin-bottom: var(--space-1);
  }

  .card {
    min-width: 0;
  }
</style>
