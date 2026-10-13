<script lang="ts">
  // Edits one traveller profile. The fields nothing reads yet stay out of the form.
  import type { TravelClass, Traveller } from "../bindings";
  import { ageGroupName, ageOn } from "../traveller";

  let {
    traveller,
    onChange,
    today,
  }: {
    traveller: Traveller;
    onChange: (traveller: Traveller) => void;
    /** The day the age group is shown for, as `YYYY-MM-DD`. */
    today: string;
  } = $props();

  const age = $derived(ageOn(traveller.birth_date, today));

  function update(changes: Partial<Traveller>) {
    onChange({ ...traveller, ...changes });
  }
</script>

<div class="fields">
  <label>
    <span class="label">Name</span>
    <input
      class="name"
      type="text"
      value={traveller.name}
      onchange={(event) => update({ name: event.currentTarget.value.trim() || traveller.name })}
    />
  </label>
  <label>
    <span class="label">Date of birth</span>
    <input
      type="date"
      class="numbers"
      value={traveller.birth_date}
      onchange={(event) => {
        if (event.currentTarget.value !== "") {
          update({ birth_date: event.currentTarget.value });
        }
      }}
    />
    <span class="hint numbers">{age} years, {ageGroupName(age)}</span>
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
</div>

<style>
  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  label {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .name {
    flex: 1;
  }

  .label {
    min-width: 8.5em;
    color: var(--color-text-muted);
  }

  .hint {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }
</style>
