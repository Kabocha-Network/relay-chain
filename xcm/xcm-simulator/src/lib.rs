// Copyright 2021 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Test kit to simulate cross-chain message passing and XCM execution

pub use codec::{Decode, Encode};
pub use paste;

pub use frame_support::{traits::Get, weights::Weight};
pub use sp_io::TestExternalities;
pub use sp_std::{cell::RefCell, marker::PhantomData};

pub use polkadot_core_primitives::BlockNumber as RelayBlockNumber;
pub use polkadot_parachain::primitives::{
	DmpMessageHandler as DmpMessageHandlerT, Id as ParaId, XcmpMessageFormat,
	XcmpMessageHandler as XcmpMessageHandlerT,
};
pub use polkadot_runtime_parachains::{
	dmp,
	ump::{self, MessageId, UmpSink, XcmSink},
};
pub use xcm::{v0::prelude::*, VersionedXcm};
pub use xcm_executor::XcmExecutor;

pub trait TestExt {
	/// Initialize the test environment.
	fn new_ext() -> sp_io::TestExternalities;
	/// Resets the state of the test environment.
	fn reset_ext();
	/// Execute some code in the context of the test externalities, with manual
	/// message processing which requires calling `process_messages()`.
	fn execute_without_auto_dispatch<R>(execute: impl FnOnce() -> R) -> R;
	/// Execute some code in the context of the test externalities, with
	/// automatic processing of messages.
	/// Messages are dispatched once the passed closure completes.
	fn execute_with<R>(execute: impl FnOnce() -> R) -> R;
}

pub enum MessageKind {
	Ump,
	Dmp,
	Xcmp,
}

pub fn encode_xcm(message: Xcm<()>, message_kind: MessageKind) -> Vec<u8> {
	match message_kind {
		MessageKind::Ump | MessageKind::Dmp => VersionedXcm::<()>::from(message).encode(),
		MessageKind::Xcmp => {
			let fmt = XcmpMessageFormat::ConcatenatedVersionedXcm;
			let mut outbound = fmt.encode();

			let encoded = VersionedXcm::<()>::from(message).encode();
			outbound.extend_from_slice(&encoded[..]);
			outbound
		},
	}
}

#[macro_export]
macro_rules! decl_test_relay_chain {
	(
		pub struct $name:ident {
			Runtime = $runtime:path,
			XcmConfig = $xcm_config:path,
			new_ext = $new_ext:expr,
		}
	) => {
		pub struct $name;

		$crate::__impl_ext!($name, $new_ext);

		impl $crate::UmpSink for $name {
			fn process_upward_message(
				origin: $crate::ParaId,
				msg: &[u8],
				max_weight: $crate::Weight,
			) -> Result<$crate::Weight, ($crate::MessageId, $crate::Weight)> {
				use $crate::{ump::UmpSink, TestExt};

				Self::execute_with(|| {
					$crate::ump::XcmSink::<$crate::XcmExecutor<$xcm_config>, $runtime>::process_upward_message(
						origin, msg, max_weight,
					)
				})
			}
		}
	};
}

#[macro_export]
macro_rules! decl_test_parachain {
	(
		pub struct $name:ident {
			Runtime = $runtime:path,
			XcmpMessageHandler = $xcmp_message_handler:path,
			DmpMessageHandler = $dmp_message_handler:path,
			new_ext = $new_ext:expr,
		}
	) => {
		pub struct $name;

		$crate::__impl_ext!($name, $new_ext);

		impl $crate::XcmpMessageHandlerT for $name {
			fn handle_xcmp_messages<
				'a,
				I: Iterator<Item = ($crate::ParaId, $crate::RelayBlockNumber, &'a [u8])>,
			>(
				iter: I,
				max_weight: $crate::Weight,
			) -> $crate::Weight {
				use $crate::{TestExt, XcmpMessageHandlerT};

				$name::execute_with(|| {
					<$xcmp_message_handler>::handle_xcmp_messages(iter, max_weight)
				})
			}
		}

		impl $crate::DmpMessageHandlerT for $name {
			fn handle_dmp_messages(
				iter: impl Iterator<Item = ($crate::RelayBlockNumber, Vec<u8>)>,
				max_weight: $crate::Weight,
			) -> $crate::Weight {
				use $crate::{DmpMessageHandlerT, TestExt};

				$name::execute_with(|| {
					<$dmp_message_handler>::handle_dmp_messages(iter, max_weight)
				})
			}
		}
	};
}

#[macro_export]
macro_rules! __impl_ext {
	// entry point: generate ext name
	($name:ident, $new_ext:expr) => {
		$crate::paste::paste! {
			$crate::__impl_ext!(@impl $name, $new_ext, [<EXT_ $name:upper>]);
		}
	};
	// impl
	(@impl $name:ident, $new_ext:expr, $ext_name:ident) => {
		thread_local! {
			pub static $ext_name: $crate::RefCell<$crate::TestExternalities>
				= $crate::RefCell::new($new_ext);
		}

		impl $crate::TestExt for $name {
			fn new_ext() -> $crate::TestExternalities {
				$new_ext
			}

			fn reset_ext() {
				$ext_name.with(|v| *v.borrow_mut() = $new_ext);
			}

			fn execute_without_auto_dispatch<R>(execute: impl FnOnce() -> R) -> R {
				$ext_name.with(|v| v.borrow_mut().execute_with(execute))
			}

			fn execute_with<R>(execute: impl FnOnce() -> R) -> R {
				let result = $ext_name.with(|v| v.borrow_mut().execute_with(execute));
				process_messages().expect("message processing failure");
				result
			}
		}
	};
}

use sp_std::collections::vec_deque::VecDeque;

thread_local! {
	pub static PARA_MESSAGE_BUS: RefCell<VecDeque<(ParaId, MultiLocation, Xcm<()>)>>
		= RefCell::new(VecDeque::new());
	pub static RELAY_MESSAGE_BUS: RefCell<VecDeque<(MultiLocation, Xcm<()>)>>
		= RefCell::new(VecDeque::new());
}

#[macro_export]
macro_rules! decl_test_network {
	(
		pub struct $name:ident {
			relay_chain = $relay_chain:ty,
			parachains = vec![ $( ($para_id:expr, $parachain:ty), )* ],
		}
	) => {
		pub struct $name;

		impl $name {
			pub fn reset() {
				use $crate::TestExt;

				<$relay_chain>::reset_ext();
				$( <$parachain>::reset_ext(); )*
			}
		}

		fn process_messages() -> $crate::XcmResult {
			process_relay_messages()?;
			process_para_messages()
		}

		/// XCM router for parachain.
		pub struct ParachainXcmRouter<T>($crate::PhantomData<T>);

		fn process_para_messages() -> $crate::XcmResult {
			use $crate::{UmpSink, XcmpMessageHandlerT};

			while let Some((para_id, destination, message)) = $crate::PARA_MESSAGE_BUS.with(
				|b| b.borrow_mut().pop_front()) {
				match destination {
					$crate::X1($crate::Parent) => {
						let encoded = $crate::encode_xcm(message, $crate::MessageKind::Ump);
						let r = <$relay_chain>::process_upward_message(
							para_id, &encoded[..],
							$crate::Weight::max_value(),
						);
						if let Err((id, required)) = r {
							return Err($crate::XcmError::WeightLimitReached(required));
						}
					},
					$(
						$crate::X2($crate::Parent, $crate::Parachain(id)) if id == $para_id => {
							let encoded = $crate::encode_xcm(message, $crate::MessageKind::Xcmp);
							let messages = vec![(para_id, 1, &encoded[..])];
							let _weight = <$parachain>::handle_xcmp_messages(
								messages.into_iter(),
								$crate::Weight::max_value(),
							);
						},
					)*
					_ => {
						return Err($crate::XcmError::CannotReachDestination(destination, message));
					}
				}
			}

			Ok(())
		}

		impl<T: $crate::Get<$crate::ParaId>> $crate::SendXcm for ParachainXcmRouter<T> {
			fn send_xcm(destination: $crate::MultiLocation, message: $crate::Xcm<()>) -> $crate::XcmResult {
				use $crate::{UmpSink, XcmpMessageHandlerT};

				match destination.clone() {
					$crate::X1($crate::Parent) => {
						$crate::PARA_MESSAGE_BUS.with(
							|b| b.borrow_mut().push_back((T::get(), destination, message)));
						Ok(())
					},
					$(
						$crate::X2($crate::Parent, $crate::Parachain(id)) if id == $para_id => {
							$crate::PARA_MESSAGE_BUS.with(
								|b| b.borrow_mut().push_back((T::get(), destination, message)));
							Ok(())
						},
					)*
					_ => Err($crate::XcmError::CannotReachDestination(destination, message)),
				}
			}
		}

		/// XCM router for relay chain.
		pub struct RelayChainXcmRouter;

		/// Process all messages originating from the relay chain.
		fn process_relay_messages() -> $crate::XcmResult {
			use $crate::DmpMessageHandlerT;

			while let Some((destination, message)) = $crate::RELAY_MESSAGE_BUS.with(
				|b| b.borrow_mut().pop_front()) {
				match destination {
					$(
						$crate::X1($crate::Parachain(id)) if id == $para_id => {
							let encoded = $crate::encode_xcm(message, $crate::MessageKind::Dmp);
							// NOTE: RelayChainBlockNumber is hard-coded to 1
							let messages = vec![(1, encoded)];
							let _weight = <$parachain>::handle_dmp_messages(
								messages.into_iter(), $crate::Weight::max_value(),
							);
						},
					)*
					_ => return Err($crate::XcmError::SendFailed("Only sends to children parachain.")),
				}
			}

			Ok(())
		}

		impl $crate::SendXcm for RelayChainXcmRouter {
			fn send_xcm(destination: $crate::MultiLocation, message: $crate::Xcm<()>) -> $crate::XcmResult {
				use $crate::DmpMessageHandlerT;

				match destination.clone() {
					$(
						$crate::X1($crate::Parachain(id)) if id == $para_id => {
							$crate::RELAY_MESSAGE_BUS.with(
								|b| b.borrow_mut().push_back((destination, message)));
							Ok(())
						},
					)*
					_ => Err($crate::XcmError::SendFailed("Only sends to children parachain.")),
				}
			}
		}
	};
}
