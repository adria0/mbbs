# Bluetooth setup
sudo apt install bluez*
sudo systemctl enable bluetooth
sudo apt install rfkill --fix-broken
sudo rfkill list
sudo hciconfig -a
sudo rfkill unblock bluetooth
sudo hciconfig hci0 up
sudo bluetoothctl
  > scan on
  > pair <MAC_ADDRESS>
