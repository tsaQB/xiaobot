use whatsapp_rust::prelude::Jid;

/// Mengambil nomor telepon numerik dari JID tanpa terpengaruh
/// sufiks perangkat (`:12`) maupun sufiks agen (`.0`).
///
/// Menggunakan field terstruktur `Jid.user` alih-alih mem-parsing
/// teks, karena representasi string JID menyertakan device dan agent
/// saat nilainya bukan nol.
pub fn jid_phone_number(jid: &Jid) -> Option<i64> {
    if !jid.server.carries_phone_number() {
        return None;
    }
    let digits: String = jid.user.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<i64>().ok().filter(|&id| id > 0)
}

/// Menentukan identitas pengirim yang stabil, dengan menoleransi
/// mode pengalamatan LID.
///
/// Pada akun LID, `source.sender` berisi ID anonim dan nomor telepon
/// asli berada di `source.sender_alt`. Kedua kolom diperiksa agar
/// pemilik tetap dikenali pada kedua mode pengalamatan.
pub fn resolve_sender_phone(sender: &Jid, sender_alt: Option<&Jid>) -> Option<i64> {
    jid_phone_number(sender).or_else(|| sender_alt.and_then(jid_phone_number))
}

/// Mengonversi format JID pengguna WhatsApp (misal "6281234567890@s.whatsapp.net")
/// menjadi integer bertanda 64-bit (`i64`) untuk kompatibilitas skema database XiaoBot.
///
/// Hanya untuk input berbentuk string murni. Untuk `Jid` terstruktur gunakan
/// [`jid_phone_number`], karena representasi string menyertakan sufiks perangkat.
pub fn parse_user_jid_to_i64(jid_str: &str) -> Option<i64> {
    let user_part = jid_str.split('@').next()?.trim();
    let digits: String = user_part.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i64>().ok().filter(|&id| id > 0)
}

/// Mengonversi format JID grup WhatsApp (misal "1203631234567890@g.us")
/// menjadi integer negatif 64-bit (`-i64`), konsisten dengan konvensi ID chat grup XiaoBot.
pub fn parse_group_jid_to_i64(jid_str: &str) -> i64 {
    let group_part = jid_str.split('@').next().unwrap_or(jid_str).trim();
    let digits: String = group_part.chars().filter(|c| c.is_ascii_digit()).collect();
    if let Ok(num) = digits.parse::<i64>() {
        -num.abs()
    } else {
        // Gunakan deterministic hash (FNV-1a) agar ID grup tidak berubah saat bot direstart
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in group_part.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let mut num = (hash as i64).saturating_abs();
        if num == 0 {
            num = 1; // Cegah ID 0
        }
        -num
    }
}

/// Memvalidasi otorisasi pengirim berdasarkan Hardened Single-Owner Boundary.
///
/// Kebijakan Keamanan (AGENTS.md §4.1):
/// Hanya nomor pemilik (`owner_number`) yang diizinkan berinteraksi dengan bot.
/// Pesan dari nomor lain diabaikan seketika (silent drop) tanpa log identitas
/// ataupun respon (*zero information leakage*).
pub fn is_sender_authorized(sender_id: i64, owner_number: Option<&str>) -> bool {
    let Some(owner) = owner_number else {
        return false;
    };

    let clean_owner: String = owner.chars().filter(|c| c.is_ascii_digit()).collect();
    if clean_owner.is_empty() {
        return false;
    }

    if let Ok(owner_id) = clean_owner.parse::<i64>() {
        sender_id == owner_id
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_jid() {
        let jid = "6281234567890@s.whatsapp.net";
        assert_eq!(parse_user_jid_to_i64(jid), Some(6281234567890));

        let invalid = "invalid_user@s.whatsapp.net";
        assert_eq!(parse_user_jid_to_i64(invalid), None);
    }

    #[test]
    fn test_parse_group_jid() {
        let group_jid = "120363028384910293@g.us";
        let group_id = parse_group_jid_to_i64(group_jid);
        assert!(group_id < 0);
        assert_eq!(group_id, -120363028384910293);
    }

    #[test]
    fn group_chat_id_is_negative_and_distinct_from_sender() {
        let sender =
            parse_user_jid_to_i64("6281234567890@s.whatsapp.net").expect("valid sender jid");
        let chat = parse_group_jid_to_i64("120363028384910293@g.us");
        assert!(sender > 0, "sender id must stay positive");
        assert!(chat < 0, "group chat id must be negative");
        assert_ne!(
            sender, chat,
            "group chat must never collapse into sender id"
        );
    }

    #[test]
    fn device_suffix_does_not_corrupt_phone_number() {
        let with_device: Jid = "6281234567890:12@s.whatsapp.net"
            .parse()
            .expect("parseable jid");
        assert_eq!(jid_phone_number(&with_device), Some(6281234567890));
    }

    #[test]
    fn lid_sender_falls_back_to_alternate_phone_jid() {
        let lid: Jid = "112233445566@lid".parse().expect("parseable lid");
        let pn: Jid = "6281234567890@s.whatsapp.net"
            .parse()
            .expect("parseable pn");
        assert_eq!(jid_phone_number(&lid), None, "lid carries no phone number");
        assert_eq!(resolve_sender_phone(&lid, Some(&pn)), Some(6281234567890));
    }

    #[test]
    fn owner_is_recognised_across_both_addressing_modes() {
        let owner = "6281234567890";
        let plain: Jid = "6281234567890@s.whatsapp.net".parse().expect("valid");
        let with_device: Jid = "6281234567890:7@s.whatsapp.net".parse().expect("valid");
        for jid in [&plain, &with_device] {
            let id = resolve_sender_phone(jid, None).expect("phone resolved");
            assert!(is_sender_authorized(id, Some(owner)));
        }
    }

    #[test]
    fn empty_owner_configuration_rejects_everyone() {
        assert!(!is_sender_authorized(6281234567890, Some("")));
        assert!(!is_sender_authorized(6281234567890, Some("   ")));
        assert!(!is_sender_authorized(6281234567890, None));
    }

    #[test]
    fn test_single_owner_authorization() {
        let owner = "6281234567890";

        // Nomor pemilik sah -> diizinkan
        assert!(is_sender_authorized(6281234567890, Some(owner)));

        // Nomor orang asing -> ditolak seketika (silent drop)
        assert!(!is_sender_authorized(6289999999999, Some(owner)));

        // Tanpa pemilik dikonfigurasi -> ditolak semua
        assert!(!is_sender_authorized(6281234567890, None));
    }
}
