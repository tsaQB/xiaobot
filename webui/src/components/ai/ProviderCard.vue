<script setup lang="ts">
import { api, seg } from '../../api/client'
import type { CatalogueRefreshResult, Ok, ProviderView } from '../../api/types'
import { L, nf } from '../../i18n'
import { confirmAction, toast } from '../../stores/ui'
import Badge from '../Badge.vue'
import BusyButton from '../BusyButton.vue'
import EffectBadge from '../EffectBadge.vue'
import Icon from '../Icon.vue'
import SecretField from '../SecretField.vue'

const props = defineProps<{ provider: ProviderView; locked?: string | null }>()
const emit = defineEmits<{ edit: []; changed: [] }>()

async function refreshCatalogue(): Promise<void> {
  const r = await api.post<CatalogueRefreshResult>(`/api/ai/providers/${seg(props.provider.id)}/models`)
  toast(L(`Katalog diperbarui: ${nf(r.models)} model (${nf(r.added)} baru)`, `Catalogue updated: ${nf(r.models)} models (${nf(r.added)} new)`))
  emit('changed')
}

function remove(): void {
  confirmAction({
    title: L('Hapus provider?', 'Delete this provider?'),
    text: L(
      'Provider dan kuncinya dihapus. Rute spesialis yang memakainya kembali ke “Ikuti model utama”.',
      'The provider and its key are deleted. Specialist routes that used it go back to “Follow the main model”.',
    ),
    label: L('Hapus', 'Delete'),
    run: async () => {
      await api.del<Ok>(`/api/ai/providers/${seg(props.provider.id)}`)
      toast(L('Provider dihapus', 'Provider deleted'))
      emit('changed')
    },
  })
}
</script>

<template>
  <div class="card">
    <div class="card-head">
      <span class="row-ic" :class="{ ok: provider.active }"><Icon name="cpu" /></span>
      <div class="grow">
        <h4>{{ provider.name }} <Badge v-if="provider.active" kind="ok">{{ L('Aktif', 'Active') }}</Badge></h4>
        <div class="sub mono">{{ provider.endpoint }}</div>
      </div>
    </div>
    <div class="card-body">
      <dl class="kv">
        <dt>{{ L('Katalog', 'Catalogue') }}</dt>
        <dd>{{ L(`${nf(provider.models.length)} model`, provider.models.length === 1 ? '1 model' : `${nf(provider.models.length)} models`) }}</dd>
        <dt>{{ L('Model aktif', 'Active model') }}</dt>
        <dd class="mono">{{ provider.active_model || '—' }}</dd>
        <dt>ID</dt>
        <dd class="mono">{{ provider.id }}</dd>
      </dl>
      <div class="field mt12">
        <div class="lab">API key <EffectBadge type="secret" /></div>
        <SecretField :secret-key="`provider:${provider.id}`" :meta="provider.key" :locked="locked" @changed="emit('changed')" />
      </div>
      <div class="actions mt12">
        <BusyButton class="btn sm" :run="refreshCatalogue" :label="L('Mengambil…', 'Fetching…')">
          <Icon name="refresh" size="sm" />{{ L('Ambil ulang katalog', 'Refresh catalogue') }}
        </BusyButton>
        <button type="button" class="btn sm" @click="emit('edit')"><Icon name="edit" size="sm" />{{ L('Ubah', 'Edit') }}</button>
        <button
          type="button"
          class="btn sm danger"
          :disabled="provider.active"
          :title="provider.active ? L('Provider aktif tidak bisa dihapus', 'The active provider cannot be removed') : undefined"
          @click="remove"
        >
          <Icon name="trash" size="sm" />{{ L('Hapus', 'Remove') }}
        </button>
      </div>
    </div>
  </div>
</template>
