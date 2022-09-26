class CreateStations < ActiveRecord::Migration[7.0]
  def change
    create_table :stations, id: :string do |t|
      t.string :name
    end
  end
end
