//! Passwd/group lookups through NSS (so it works with anything NixOS configures).

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

fn buf_size(sc: libc::c_int) -> usize {
    let n = unsafe { libc::sysconf(sc) };
    if n > 0 {
        n as usize
    } else {
        16 * 1024
    }
}

/// `(uid, primary gid)` of `name`, if the user exists.
pub fn lookup_user(name: &str) -> Option<(libc::uid_t, libc::gid_t)> {
    let cname = CString::new(name).ok()?;
    let mut size = buf_size(libc::_SC_GETPW_R_SIZE_MAX);
    loop {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let mut buf = vec![0 as c_char; size];
        let rc = unsafe {
            libc::getpwnam_r(
                cname.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && size < (1 << 20) {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        return Some((pwd.pw_uid, pwd.pw_gid));
    }
}

fn group_name(gid: libc::gid_t) -> Option<String> {
    let mut size = buf_size(libc::_SC_GETGR_R_SIZE_MAX);
    loop {
        let mut grp: libc::group = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::group = std::ptr::null_mut();
        let mut buf = vec![0 as c_char; size];
        let rc =
            unsafe { libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut result) };
        if rc == libc::ERANGE && size < (1 << 20) {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(grp.gr_name) };
        return Some(name.to_string_lossy().into_owned());
    }
}

/// Names of every group `name` belongs to (primary included), or `None` if the
/// user does not exist.
pub fn user_groups(name: &str) -> Option<Vec<String>> {
    let (_, gid) = lookup_user(name)?;
    let cname = CString::new(name).ok()?;

    let mut ngroups: libc::c_int = 32;
    let mut gids: Vec<libc::gid_t> = vec![0; ngroups as usize];
    loop {
        let rc =
            unsafe { libc::getgrouplist(cname.as_ptr(), gid, gids.as_mut_ptr(), &mut ngroups) };
        if rc >= 0 {
            gids.truncate(ngroups as usize);
            break;
        }
        // rc == -1: buffer too small, ngroups now holds the required size.
        if ngroups as usize <= gids.len() || ngroups > 4096 {
            return None;
        }
        gids.resize(ngroups as usize, 0);
    }

    let mut names: Vec<String> = gids.into_iter().filter_map(group_name).collect();
    names.sort();
    names.dedup();
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_exists_and_has_root_group() {
        assert_eq!(lookup_user("root").map(|(uid, _)| uid), Some(0));
        let groups = user_groups("root").expect("root exists");
        assert!(groups.iter().any(|g| g == "root"), "got {groups:?}");
    }

    #[test]
    fn missing_user_is_none() {
        assert!(lookup_user("vexnas-no-such-user-xyz").is_none());
        assert!(user_groups("vexnas-no-such-user-xyz").is_none());
    }
}
